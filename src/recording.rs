use std::{
    cell::{Cell, OnceCell, RefCell},
    error,
    ffi::OsStr,
    fmt,
    os::fd::{IntoRawFd, OwnedFd},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use anyhow::{Context, Error, Result, ensure};
use gettextrs::gettext;
use gst::prelude::*;
use gtk::{
    gio::{self, prelude::*},
    glib::{self, clone, closure_local, subclass::prelude::*},
};

use crate::{
    application::Application,
    area_selector::AreaSelector,
    cancelled::Cancelled,
    capture_warnings::CaptureWarnings,
    experimental::Feature,
    help::ContextWithHelp,
    i18n::gettext_f,
    pause_gate::PauseGate,
    pipeline::PipelineBuilder,
    profile::Profile,
    screencast_portal::{
        CursorMode, PersistMode, Proxy, Session, SourceType, Stream, WindowIdentifier,
    },
    settings::{CaptureMode, Settings},
    timer::Timer,
};

const DURATION_UPDATE_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub struct NoProfileError;

impl fmt::Display for NoProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&gettext("No active profile"))
    }
}

impl error::Error for NoProfileError {}

/// Everything a recording reads from the settings, read once when it starts.
// The bools mirror independent settings keys, not a hidden state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub profile: &'static Profile,
    pub record_desktop_audio: bool,
    pub record_microphone: bool,
    pub show_pointer: bool,
    pub capture_mode: CaptureMode,
    pub framerate: gst::Fraction,
    pub record_delay: Duration,
    pub saving_location: PathBuf,
    pub pause_flash: bool,
}

impl CaptureConfig {
    pub fn from_settings(settings: &Settings) -> Result<Self> {
        Ok(Self {
            profile: settings.profile().context(NoProfileError)?,
            record_desktop_audio: settings.record_desktop_audio(),
            record_microphone: settings.record_microphone(),
            show_pointer: settings.show_pointer(),
            capture_mode: settings.capture_mode(),
            framerate: settings.framerate(),
            record_delay: settings.record_delay(),
            saving_location: settings.saving_location(),
            pause_flash: settings.pause_flash(),
        })
    }

    pub fn warnings(&self) -> CaptureWarnings {
        CaptureWarnings::compute(
            self.profile.supports_audio(),
            self.record_desktop_audio,
            self.record_microphone,
            self.show_pointer,
        )
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "KoohaRecordingState")]
pub enum RecordingState {
    #[default]
    Init,
    Delayed {
        secs_left: u64,
    },
    Recording,
    Paused,
    Flushing {
        /// Progress in percentage with range 0..=100
        progress: u8,
    },
    Finished,
}

#[derive(Debug, Clone, glib::SharedBoxed)]
#[shared_boxed_type(name = "KoohaRecordingResult")]
struct BoxedResult(Rc<Result<(gio::File, gst::ClockTime)>>);

mod imp {
    use std::sync::LazyLock;

    use glib::subclass::Signal;
    use gst::bus::BusWatchGuard;

    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::Recording)]
    pub struct Recording {
        #[property(get)]
        pub(super) state: Cell<RecordingState>,
        #[property(get)]
        pub(super) duration: Cell<gst::ClockTime>,
        #[property(get)]
        pub(super) capture_warnings: Cell<CaptureWarnings>,

        pub(super) file: OnceCell<gio::File>,

        pub(super) estimated_final_duration: Cell<Option<gst::ClockTime>>,
        pub(super) pause_gate: PauseGate,

        pub(super) timer: RefCell<Option<Timer>>,
        pub(super) session: RefCell<Option<Session>>,
        pub(super) duration_source_id: RefCell<Option<glib::SourceId>>,
        pub(super) pipeline: OnceCell<gst::Pipeline>,
        pub(super) bus_watch_guard: RefCell<Option<BusWatchGuard>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Recording {
        const NAME: &'static str = "KoohaRecording";
        type Type = super::Recording;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Recording {
        fn dispose(&self) {
            if let Some(timer) = self.timer.take() {
                timer.cancel();
            }

            if let Some(pipeline) = self.pipeline.get()
                && let Err(err) = pipeline.set_state(gst::State::Null)
            {
                tracing::warn!("Failed to stop pipeline on dispose: {:?}", err);
            }

            self.obj().close_session();

            if let Some(source_id) = self.duration_source_id.take() {
                source_id.remove();
            }
        }

        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("finished")
                        .param_types([BoxedResult::static_type()])
                        .build(),
                ]
            });

            SIGNALS.as_ref()
        }
    }
}

glib::wrapper! {
     pub struct Recording(ObjectSubclass<imp::Recording>);
}

impl Recording {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub async fn start(&self, parent: Option<&impl IsA<gtk::Window>>, settings: &Settings) {
        if !matches!(self.state(), RecordingState::Init) {
            tracing::error!("Trying to start recording on a non-init state");
            return;
        }

        if let Err(err) = self.start_inner(parent, settings).await {
            self.close_session();
            self.set_finished(Err(err));
        }
    }

    async fn start_inner(
        &self,
        parent: Option<&impl IsA<gtk::Window>>,
        settings: &Settings,
    ) -> Result<()> {
        let imp = self.imp();
        let config = CaptureConfig::from_settings(settings)?;

        imp.capture_warnings.set(config.warnings());
        self.notify_capture_warnings();

        // Setup screencast session
        let restore_token = settings.screencast_restore_token();
        settings.reset_screencast_restore_token();
        let (screencast_session, streams, restore_token, fd) = new_screencast_session(
            if config.show_pointer {
                CursorMode::EMBEDDED
            } else {
                CursorMode::HIDDEN
            },
            if Feature::WindowRecording.is_enabled() {
                SourceType::MONITOR | SourceType::WINDOW
            } else {
                SourceType::MONITOR
            },
            Feature::MultipleVideoSources.is_enabled(),
            Some(&restore_token),
            PersistMode::Persistent,
            parent,
        )
        .await
        .with_context(|| {
            ContextWithHelp::new(
                gettext("Failed to start recording"),
                gettext_f(
                    // Translators: Do NOT translate the contents between '{' and '}', this is a variable name.
                    "Check out {link} for help.",
                    &[("link", r#"<a href="https://github.com/SeaDve/Kooha#-it-doesnt-work">It Doesn't Work page</a>"#)],
                ),
            )
        })?;

        imp.session.replace(Some(screencast_session));
        settings.set_screencast_restore_token(&restore_token.unwrap_or_default());

        let file_path =
            new_recording_path(&config.saving_location, config.profile.file_extension());
        imp.file.set(gio::File::for_path(&file_path)).unwrap();

        let raw_fd = fd.into_raw_fd();

        let mut pipeline_builder = PipelineBuilder::new(
            file_path,
            config.framerate,
            config.profile.clone(),
            raw_fd,
            streams.clone(),
        );

        // Select area
        if config.capture_mode == CaptureMode::Selection {
            let data = AreaSelector::select(raw_fd, &streams, &Application::get().window()).await?;
            pipeline_builder.select_area_data(data);
        }

        if config.profile.supports_audio() {
            pipeline_builder.record_desktop_audio(config.record_desktop_audio);
            pipeline_builder.record_microphone(config.record_microphone);
        }

        pipeline_builder
            .pause_gate(imp.pause_gate.clone())
            .pause_flash(config.pause_flash);

        // Build pipeline
        let pipeline = pipeline_builder.build().with_context(|| {
            ContextWithHelp::new(
                gettext("Failed to start recording"),
                gettext("A GStreamer plugin may not be installed."),
            )
        })?;
        imp.pipeline.set(pipeline.clone()).unwrap();

        // Setup and run timer
        let timer = Timer::new(
            config.record_delay,
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |secs_left| {
                    obj.set_state(RecordingState::Delayed { secs_left });
                }
            ),
        );
        imp.timer.replace(Some(Timer::clone(&timer)));
        timer.await?;

        self.play()
    }

    /// Watches the bus, starts updating the duration and sets the pipeline
    /// to playing.
    fn play(&self) -> Result<()> {
        let imp = self.imp();
        let pipeline = self.pipeline();

        let bus_watch_guard = pipeline
            .bus()
            .unwrap()
            .add_watch_local(clone!(
                #[weak(rename_to = obj)]
                self,
                #[upgrade_or_panic]
                move |_, message| obj.handle_bus_message(message)
            ))
            .unwrap();
        imp.bus_watch_guard.replace(Some(bus_watch_guard));
        imp.duration_source_id.replace(Some(glib::timeout_add_local(
            DURATION_UPDATE_INTERVAL,
            clone!(
                #[weak(rename_to = obj)]
                self,
                #[upgrade_or_panic]
                move || {
                    obj.update_duration();
                    obj.update_flushing_progress();
                    glib::ControlFlow::Continue
                }
            ),
        )));

        pipeline
            .set_state(gst::State::Playing)
            .context("Failed to initialize pipeline state to playing")
            .with_context(|| {
                ContextWithHelp::new(
                    gettext("Failed to start recording"),
                    gettext("Make sure that the saving location exists and is accessible."),
                )
            })?;

        self.update_duration();

        Ok(())
    }

    /// Pauses with the pause gate; the pipeline stays playing (ADR 0003).
    pub fn pause(&self) -> Result<()> {
        ensure!(
            matches!(self.state(), RecordingState::Recording),
            "Recording can only be paused from recording state"
        );

        let running_time = self
            .pipeline()
            .current_running_time()
            .context("Failed to pause: the pipeline has no running time")?;
        self.imp().pause_gate.pause(running_time);
        self.set_state(RecordingState::Paused);
        self.update_duration();

        Ok(())
    }

    pub fn resume(&self) -> Result<()> {
        ensure!(
            matches!(self.state(), RecordingState::Paused),
            "Recording can only be resumed from paused state"
        );

        let running_time = self
            .pipeline()
            .current_running_time()
            .context("Failed to resume: the pipeline has no running time")?;
        self.imp().pause_gate.resume(running_time);
        self.set_state(RecordingState::Recording);
        self.update_duration();

        Ok(())
    }

    pub fn stop(&self) {
        let imp = self.imp();

        let state = self.state();
        if matches!(
            state,
            RecordingState::Init | RecordingState::Flushing { .. } | RecordingState::Finished
        ) {
            tracing::error!("Trying to stop recording on a `{:?}` state", state);
            return;
        }

        let pipeline = self.pipeline();

        // Taken before flushing, while the duration still excludes the pauses.
        imp.estimated_final_duration.set(
            pipeline
                .current_running_time()
                .map(|running_time| imp.pause_gate.recorded_duration(running_time)),
        );
        self.set_state(RecordingState::Flushing { progress: 0 });
        // While flushing, the duration is what the pipeline has processed.
        self.update_duration();
        self.update_flushing_progress();

        tracing::debug!("Sending eos event to pipeline");
        // FIXME Maybe it is needed to verify if we received the same
        // eos event by checking its seqnum in the bus?
        pipeline.send_event(gst::event::Eos::new());
    }

    pub fn cancel(&self) {
        let imp = self.imp();

        tracing::debug!("Cancelling recording");

        if let Some(timer) = imp.timer.take() {
            timer.cancel();
        }

        if let Some(pipeline) = imp.pipeline.get()
            && let Err(err) = pipeline.set_state(gst::State::Null)
        {
            tracing::warn!("Failed to stop pipeline on cancel: {:?}", err);
        }

        let _ = imp.bus_watch_guard.take();

        self.close_session();

        if let Some(source_id) = imp.duration_source_id.take() {
            source_id.remove();
        }

        // HACK we need to return before calling this to avoid a `BorrowMutError` when
        // `Window` tried to take the `recording` on finished callback while `recording`
        // is borrowed to call `cancel`.
        glib::idle_add_local_once(clone!(
            #[weak(rename_to = obj)]
            self,
            move || {
                obj.set_finished(Err(Error::from(Cancelled::new("recording"))));
            }
        ));
    }

    pub fn connect_finished<F>(&self, f: F) -> glib::SignalHandlerId
    where
        F: Fn(&Self, &Result<(gio::File, gst::ClockTime)>) + 'static,
    {
        self.connect_closure(
            "finished",
            true,
            closure_local!(|obj: &Self, result: BoxedResult| {
                f(obj, &result.0);
            }),
        )
    }

    fn set_state(&self, state: RecordingState) {
        tracing::trace!("Recording state changed to {:?}", state);

        if state == self.state() {
            return;
        }

        self.imp().state.replace(state);
        self.notify_state();
    }

    fn file(&self) -> &gio::File {
        self.imp()
            .file
            .get()
            .expect("file not set, make sure to start recording first")
    }

    fn pipeline(&self) -> &gst::Pipeline {
        self.imp()
            .pipeline
            .get()
            .expect("pipeline not set, make sure to start recording first")
    }

    fn set_finished(&self, res: Result<(gio::File, gst::ClockTime)>) {
        self.set_state(RecordingState::Finished);

        let result = BoxedResult(Rc::new(res));
        self.emit_by_name::<()>("finished", &[&result]);
    }

    /// Closes session on the background
    fn close_session(&self) {
        if let Some(session) = self.imp().session.take() {
            glib::spawn_future_local(async move {
                if let Err(err) = session.close().await {
                    tracing::warn!("Failed to close screencast session: {:?}", err);
                }
            });
        }
    }

    fn update_duration(&self) {
        let imp = self.imp();

        let clock_time = imp
            .pipeline
            .get()
            .and_then(|pipeline| match self.state() {
                // Time on the clock, without the pauses.
                RecordingState::Recording | RecordingState::Paused => pipeline
                    .current_running_time()
                    .map(|running_time| imp.pause_gate.recorded_duration(running_time)),
                // What the pipeline has already processed.
                _ => pipeline.query_position::<gst::ClockTime>(),
            })
            .unwrap_or(gst::ClockTime::ZERO);

        if clock_time == self.duration() {
            return;
        }

        imp.duration.set(clock_time);
        self.notify_duration();
    }

    fn update_flushing_progress(&self) {
        let imp = self.imp();

        let RecordingState::Flushing {
            progress: prev_progress,
        } = self.state()
        else {
            return;
        };

        let progress = imp
            .estimated_final_duration
            .get()
            .map_or(0, |estimated_final_duration| {
                flushing_progress(self.duration(), estimated_final_duration)
            });

        if prev_progress == progress {
            return;
        }

        self.set_state(RecordingState::Flushing { progress });
    }

    fn handle_bus_message(&self, message: &gst::Message) -> glib::ControlFlow {
        use gst::MessageView;

        let imp = self.imp();

        match message.view() {
            MessageView::Error(e) => {
                tracing::debug!(state = ?self.state(), "Received error at bus");

                if let Err(err) = self.pipeline().set_state(gst::State::Null) {
                    tracing::warn!("Failed to stop pipeline on error: {:?}", err);
                }

                self.close_session();

                if let Some(source_id) = imp.duration_source_id.take() {
                    source_id.remove();
                }

                // TODO print error quarks for all glib::Error

                let error = Error::from(e.error())
                    .context(e.debug().unwrap_or_else(|| "<no debug>".into()))
                    .context(gettext("An error occurred while recording"));

                let error = if e.error().matches(gst::ResourceError::OpenWrite) {
                    error.context(ContextWithHelp::new(
                        gettext_f(
                            // Translators: Do NOT translate the contents between '{' and '}', this is a variable name.
                            "Failed to open “{path}” for writing",
                            &[("path", &self.file().uri())],
                        ),
                        gettext("Make sure that the saving location exists and is accessible."),
                    ))
                } else {
                    error
                };

                self.set_finished(Err(error));

                glib::ControlFlow::Break
            }
            MessageView::Eos(..) => {
                tracing::debug!("Eos signal received from record bus");

                debug_assert!(
                    matches!(self.state(), RecordingState::Flushing { .. }),
                    "received eos signal on {:?}",
                    self.state()
                );

                self.set_state(RecordingState::Flushing { progress: 100 });

                self.update_duration();
                let duration = self.duration();

                if let Err(err) = self.pipeline().set_state(gst::State::Null) {
                    tracing::error!("Failed to stop pipeline on eos: {:?}", err);
                }

                self.close_session();

                if let Some(source_id) = imp.duration_source_id.take() {
                    source_id.remove();
                }

                self.set_finished(Ok((self.file().clone(), duration)));

                glib::ControlFlow::Break
            }
            MessageView::StateChanged(sc) => {
                let new_state = sc.current();

                if message.src()
                    != imp
                        .pipeline
                        .get()
                        .map(|pipeline| pipeline.upcast_ref::<gst::Object>())
                {
                    tracing::trace!(
                        "`{}` changed state from `{:?}` -> `{:?}`",
                        message
                            .src()
                            .map_or_else(|| "<unknown source>".into(), |e| e.name()),
                        sc.old(),
                        new_state,
                    );
                    return glib::ControlFlow::Continue;
                }

                tracing::debug!(
                    "Pipeline changed state from `{:?}` -> `{:?}`",
                    sc.old(),
                    new_state,
                );

                // The pause gate sets `Paused`; the pipeline stays playing.
                if new_state == gst::State::Playing
                    && matches!(
                        self.state(),
                        RecordingState::Init | RecordingState::Delayed { .. }
                    )
                {
                    self.set_state(RecordingState::Recording);
                }

                glib::ControlFlow::Continue
            }
            MessageView::AsyncDone(ad) => {
                tracing::trace!("Received async-done message on bus: {:?}", ad);

                // This is enabled by setting `GST_DEBUG_DUMP_DOT_DIR` to a directory (e.g. `GST_DEBUG_DUMP_DOT_DIR=.`).
                self.pipeline().debug_to_dot_file_with_ts(
                    gst::DebugGraphDetails::VERBOSE,
                    "kooha-recording-pipeline",
                );

                glib::ControlFlow::Continue
            }
            MessageView::Warning(w) => {
                tracing::warn!("Received warning message on bus: {:?}", w);
                glib::ControlFlow::Continue
            }
            MessageView::Info(i) => {
                tracing::debug!("Received info message on bus: {:?}", i);
                glib::ControlFlow::Continue
            }
            MessageView::Qos(q) => {
                tracing::warn!("Received qos message on bus: {:?}", q);
                glib::ControlFlow::Continue
            }
            other => {
                tracing::trace!("Received other message on bus: {:?}", other);
                glib::ControlFlow::Continue
            }
        }
    }
}

async fn new_screencast_session(
    cursor_mode: CursorMode,
    source_type: SourceType,
    is_multiple_sources: bool,
    restore_token: Option<&str>,
    persist_mode: PersistMode,
    parent_window: Option<&impl IsA<gtk::Window>>,
) -> Result<(Session, Vec<Stream>, Option<String>, OwnedFd)> {
    let proxy = Proxy::new()
        .await
        .context("Failed to create screencast proxy")?;

    tracing::debug!(
        version = ?proxy.version(),
        available_cursor_modes = ?proxy.available_cursor_modes(),
        available_source_types = ?proxy.available_source_types(),
        "Created screencast proxy"
    );

    let session = proxy
        .create_session()
        .await
        .context("Failed to create session")?;

    tracing::debug!(
        ?cursor_mode,
        ?source_type,
        is_multiple_sources,
        restore_token,
        ?persist_mode,
        "Selecting sources"
    );

    session
        .select_sources(
            source_type,
            is_multiple_sources,
            cursor_mode,
            restore_token,
            persist_mode,
        )
        .await
        .context("Failed to select sources")?;

    let window_identifier = if let Some(window) = parent_window {
        WindowIdentifier::new(window.as_ref()).await
    } else {
        WindowIdentifier::None
    };

    tracing::debug!(%window_identifier, "Starting session");

    let (streams, restore_token) = session
        .start(window_identifier)
        .await
        .context("Failed to start session")?;

    let fd = session
        .open_pipe_wire_remote()
        .await
        .context("Failed to open pipewire remote")?;

    Ok((session, streams, restore_token, fd))
}

/// Saving progress in percent: `processed` out of `estimated`, capped at 100.
fn flushing_progress(processed: gst::ClockTime, estimated: gst::ClockTime) -> u8 {
    if estimated.is_zero() {
        return 0;
    }

    let percent = processed.nseconds() as f64 / estimated.nseconds() as f64 * 100.0;
    percent.round().min(100.0) as u8
}

fn new_recording_path(saving_location: &Path, extension: impl AsRef<OsStr>) -> PathBuf {
    let file_name = glib::DateTime::now_local()
        .expect("You are somehow on year 9999")
        .format("Kooha-%F-%H-%M-%S")
        .expect("Invalid format string");

    let mut path = saving_location.join(file_name);
    path.set_extension(extension);

    path
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::env;

    use crate::settings::test_utils::{
        init_resources, settings_lock, setup_schema, temp_saving_location,
    };

    #[test]
    fn config_is_a_snapshot() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        for key in [
            "profile-id",
            "record-desktop-audio",
            "record-microphone",
            "show-pointer",
            "pause-flash",
        ] {
            settings.reset(key);
        }
        let saving_location = temp_saving_location(&settings);
        settings.set_record_desktop_audio(true);
        settings.set_record_microphone(false);
        settings.set_pause_flash(true);

        let config = CaptureConfig::from_settings(&settings).unwrap();
        settings.set_record_microphone(true);
        settings.set_pause_flash(false);

        assert!(!config.record_microphone);
        assert!(config.pause_flash);
        assert_eq!(config.saving_location, saving_location);
        assert_eq!(config.warnings().labels(), ["Without microphone"]);

        settings.reset("saving-location");
        settings.reset("pause-flash");
    }

    #[test]
    fn flushing_progress_rounds_and_caps() {
        let ms = gst::ClockTime::from_mseconds;

        assert_eq!(flushing_progress(ms(500), gst::ClockTime::ZERO), 0);
        assert_eq!(flushing_progress(ms(0), ms(1000)), 0);
        assert_eq!(flushing_progress(ms(500), ms(1000)), 50);
        assert_eq!(flushing_progress(ms(554), ms(1000)), 55);
        assert_eq!(flushing_progress(ms(555), ms(1000)), 56);
        assert_eq!(flushing_progress(ms(1000), ms(1000)), 100);
        assert_eq!(flushing_progress(ms(2000), ms(1000)), 100);
    }

    /// Runs `pipeline` as the recording's, skipping the portal and the timer.
    fn run_pipeline(recording: &Recording, pipeline: gst::Pipeline, file: gio::File) {
        let imp = recording.imp();
        imp.pipeline.set(pipeline).unwrap();
        imp.file.set(file).unwrap();
        recording.play().unwrap();
    }

    /// Waits on the main context until `condition` holds, for 5 s at most.
    async fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            glib::timeout_future(Duration::from_millis(10)).await;
        }
    }

    /// Records with a synthetic pipeline that lags behind its source (each
    /// buffer takes 40 ms to go through, frames come every 33 ms), stops,
    /// and returns the progress right after `stop()` and every progress the
    /// recording went through while flushing.
    async fn record_lagging(pause_before_stop: bool) -> (RecordingState, Vec<u8>) {
        gst::init().unwrap();

        let pipeline = gst::parse::launch(
            "videotestsrc is-live=true ! video/x-raw,framerate=30/1 ! identity name=gate ! queue ! identity sleep-time=40000 ! fakesink sync=false",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();

        let recording = Recording::new();
        recording
            .imp()
            .pause_gate
            .attach(&pipeline.by_name("gate").unwrap().static_pad("src").unwrap());

        let progress = Rc::new(RefCell::new(Vec::new()));
        recording.connect_state_notify(clone!(
            #[strong]
            progress,
            move |recording| {
                if let RecordingState::Flushing { progress: value } = recording.state() {
                    progress.borrow_mut().push(value);
                }
            }
        ));
        let result = Rc::new(RefCell::new(None));
        recording.connect_finished(clone!(
            #[strong]
            result,
            move |_, res| {
                result.replace(Some(res.as_ref().map(|(_, duration)| *duration).ok()));
            }
        ));

        run_pipeline(
            &recording,
            pipeline,
            gio::File::for_path(env::temp_dir().join("kooha-test-lagging")),
        );
        wait_until("recording", || {
            recording.state() == RecordingState::Recording
        })
        .await;

        glib::timeout_future(Duration::from_secs(1)).await;
        if pause_before_stop {
            recording.pause().unwrap();
            assert_eq!(recording.state(), RecordingState::Paused);
            // Shorter than the lag, so there is still something to flush.
            glib::timeout_future(Duration::from_millis(50)).await;
        }

        recording.stop();
        let after_stop = recording.state();

        wait_until("the end of the recording", || result.borrow().is_some()).await;
        assert!(result.borrow().unwrap().is_some(), "the recording failed");
        assert_eq!(recording.state(), RecordingState::Finished);

        let progress = progress.borrow().clone();
        (after_stop, progress)
    }

    #[track_caller]
    fn assert_progress(after_stop: RecordingState, progress: &[u8]) {
        let RecordingState::Flushing {
            progress: progress_after_stop,
        } = after_stop
        else {
            panic!("not flushing after stop: {after_stop:?}");
        };
        assert!(progress_after_stop < 100, "{progress:?}");
        assert!(
            progress.windows(2).all(|pair| pair[0] <= pair[1]),
            "{progress:?}"
        );
        assert_eq!(progress.last(), Some(&100), "{progress:?}");
    }

    #[gtk::test]
    async fn flushing_progress_grows_while_lagging() {
        let (after_stop, progress) = record_lagging(false).await;

        assert_progress(after_stop, &progress);
    }

    #[gtk::test]
    async fn flushing_progress_grows_when_stopped_paused() {
        let (after_stop, progress) = record_lagging(true).await;

        assert_progress(after_stop, &progress);
    }
}
