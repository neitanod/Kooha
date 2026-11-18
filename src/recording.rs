use std::{
    cell::{Cell, OnceCell, RefCell},
    error,
    ffi::OsStr,
    fmt,
    future::Future,
    os::fd::{AsRawFd, IntoRawFd, OwnedFd},
    path::{Path, PathBuf},
    pin::Pin,
    rc::Rc,
    time::Duration,
};

use anyhow::{Context, Error, Result, ensure};
use gettextrs::gettext;
use gst::prelude::*;
use gtk::{
    gdk,
    gio::{self, prelude::*},
    glib::{self, clone, closure_local, subclass::prelude::*},
};

use crate::{
    application::Application,
    area_border::{self, geometry::BorderGeometry},
    area_selector::AreaSelector,
    areas::{self, ChosenArea, MarkedArea, MonitorInfo},
    cancelled::Cancelled,
    capture_warnings::CaptureWarnings,
    encoder::{self, H264Encoder},
    experimental::Feature,
    help::ContextWithHelp,
    i18n::gettext_f,
    overlay,
    pause_gate::{FlashColor, PauseGate},
    pipeline::{AreaCrop, PipelineBuilder, compute_crop},
    profile::Profile,
    screencast_portal::{
        CursorMode, PersistMode, Proxy, Session, SourceType, Stream, WindowIdentifier,
    },
    settings::{CaptureMode, Settings, VideoEncoder},
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
    pub pause_flash_color: FlashColor,
    /// The area to record without the upstream selector, from
    /// `area_to_record`.
    pub area: Option<ChosenArea>,
    /// Used only by the profiles that encode H.264 (ADR 0006).
    pub encoder: VideoEncoder,
}

impl CaptureConfig {
    pub fn from_settings(settings: &Settings, area: Option<ChosenArea>) -> Result<Self> {
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
            pause_flash_color: settings.pause_flash_color().into(),
            area,
            encoder: settings.video_encoder(),
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

/// The H.264 encoder to record `profile` with, or `None` when the profile
/// does not encode H.264, which never runs `works`. A forced encoder that
/// does not work is an error that says how to fix it (ADR 0006).
pub async fn h264_encoder_for<F, Fut>(
    profile: &Profile,
    pref: VideoEncoder,
    works: F,
) -> Result<Option<H264Encoder>>
where
    F: Fn(H264Encoder) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if !profile.uses_h264_encoder() {
        return Ok(None);
    }

    match encoder::resolve(pref, works).await {
        Ok(encoder) => Ok(Some(encoder)),
        Err(reason) => {
            // Automatic fails only when its last candidate, the CPU, does.
            let failed = encoder::candidates(pref)
                .last()
                .copied()
                .unwrap_or(H264Encoder::Cpu);
            Err(Error::msg(reason).context(encoder_help(failed)))
        }
    }
}

/// What to tell when `encoder` does not work.
fn encoder_help(encoder: H264Encoder) -> ContextWithHelp {
    ContextWithHelp::new(
        gettext_f(
            // Translators: Do NOT translate the contents between '{' and '}', this is a variable name.
            "The {encoder} encoder is not working",
            &[("encoder", encoder.name())],
        ),
        gettext("Choose Automatic or CPU in Preferences."),
    )
}

/// The help for an error of the recording's pipeline posted by `src`, when
/// `src` is the element that encodes with `encoder`, the pipeline's
/// encoder. The probe runs at a fixed size for whole streams, so an encoder
/// can still fail with the size of the real frames.
///
/// The converters and caps filters of the chain are left out: the pipeline
/// has others, whose errors are not the encoder's (fable code r2 #3).
fn encoder_error_help(
    encoder: Option<H264Encoder>,
    src: Option<&gst::Object>,
) -> Option<ContextWithHelp> {
    let encoder = encoder?;
    let factory = src?.downcast_ref::<gst::Element>()?.factory()?;
    (factory.name() == encoder.encoder_element()).then(|| encoder_help(encoder))
}

/// Whether recording in `mode` has to mark an area first: in Selection mode
/// with the marker's layers available, when no area is chosen. Without
/// layers, the upstream selector picks the area (ADR 0004).
pub fn needs_marking(mode: CaptureMode, overlays_available: bool, has_area: bool) -> bool {
    mode == CaptureMode::Selection && overlays_available && !has_area
}

/// The area to record: the `chosen` one in Selection mode with the
/// marker's layers available, else none and the upstream selector.
pub fn area_to_record(
    mode: CaptureMode,
    overlays_available: bool,
    chosen: impl FnOnce() -> Option<ChosenArea>,
) -> Option<ChosenArea> {
    if mode == CaptureMode::Selection && overlays_available {
        chosen()
    } else {
        None
    }
}

/// The chosen area is out of date: its monitor is gone or changed (P10).
#[derive(Debug, Clone, PartialEq)]
pub struct StaleArea {
    /// `None` for the last area, which has no name.
    pub name: Option<String>,
    /// Why, like `DP-3 is not connected`.
    pub reason: String,
}

impl StaleArea {
    /// What the toast says: the heading and the reason.
    pub fn message(&self) -> String {
        format!("{self}: {}", self.reason)
    }
}

impl fmt::Display for StaleArea {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => f.write_str(&gettext_f(
                "The area “{name}” is out of date",
                &[("name", name)],
            )),
            None => f.write_str(&gettext("The last area is out of date")),
        }
    }
}

impl error::Error for StaleArea {}

/// The stream shared is not the monitor of the area, twice (P9).
#[derive(Debug, Clone, PartialEq)]
pub struct WrongMonitor {
    /// The size of the stream, if a single one with a size was shared.
    pub shared: Option<(i32, i32)>,
    pub connector: String,
    /// The physical size of the area's monitor.
    pub monitor: (i32, i32),
}

impl fmt::Display for WrongMonitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (monitor_width, monitor_height) = self.monitor;
        let monitor = format!("{monitor_width}×{monitor_height}");
        let message = match self.shared {
            Some((width, height)) => gettext_f(
                "You shared {shared}, but the area is on {connector} ({monitor})",
                &[
                    ("shared", &format!("{width}×{height}")),
                    ("connector", &self.connector),
                    ("monitor", &monitor),
                ],
            ),
            None => gettext_f(
                "You shared more than one screen, but the area is on {connector} ({monitor})",
                &[("connector", &self.connector), ("monitor", &monitor)],
            ),
        };
        f.write_str(&message)
    }
}

impl error::Error for WrongMonitor {}

/// How far, per axis, a stream may be from a physical monitor size: logical
/// sizes times a fractional scale round differently.
const STREAM_SIZE_TOLERANCE: i32 = 2;

/// How far, per axis, a stream may be from a logical monitor size.
const LOGICAL_SIZE_TOLERANCE: i32 = 1;

/// How a whole-monitor stream relates to the monitor of the area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamMatch {
    /// The stream is the area's monitor.
    Same,
    /// Another monitor, of `shared` size, or more than one stream.
    Other { shared: Option<(i32, i32)> },
    /// The stream doesn't say its size.
    Unknown,
}

/// Whether `streams` capture the monitor of `area`, by size (ADR 0005): the
/// physical size the area was marked on, ±2, or the logical size of that
/// monitor among `monitors`, ±1, as the portal spec says.
pub(crate) fn match_stream(
    streams: &[Stream],
    area: &MarkedArea,
    monitors: &[MonitorInfo],
) -> StreamMatch {
    let [stream] = streams else {
        return StreamMatch::Other { shared: None };
    };
    let Some(size) = stream.size() else {
        tracing::warn!("The stream doesn't say its size; assuming it is the area's monitor");
        return StreamMatch::Unknown;
    };

    let near = |a: (i32, i32), b: (i32, i32), tolerance: i32| {
        (a.0 - b.0).abs() <= tolerance && (a.1 - b.1).abs() <= tolerance
    };
    let matches = |monitor: &MonitorInfo| {
        let (_, _, width, height) = monitor.logical;
        near(size, monitor.physical_size(), STREAM_SIZE_TOLERANCE)
            || near(size, (width, height), LOGICAL_SIZE_TOLERANCE)
    };

    let marked_on = (area.monitor_width, area.monitor_height);
    let own = monitors
        .iter()
        .find(|monitor| monitor.connector == area.connector);
    if !near(size, marked_on, STREAM_SIZE_TOLERANCE) && !own.is_some_and(matches) {
        return StreamMatch::Other { shared: Some(size) };
    }

    // Monitors of the same size can't be told apart (ADR 0005).
    if monitors
        .iter()
        .any(|monitor| monitor.connector != area.connector && matches(monitor))
    {
        tracing::warn!(
            ?size,
            connector = area.connector,
            "Another monitor has the size of the stream; assuming the area's"
        );
    }
    StreamMatch::Same
}

/// Opens a screencast session for `area`: if the shared stream is another
/// monitor, closes that session, forgets the restore token, tells the user
/// which monitor to choose with `notice`, and opens a second one without
/// the token. Once the second one answers, `withdraw_notice` takes the
/// notice back, as it could show in the video (fable code r2 #2). A second
/// mismatch is a `WrongMonitor` error. The restore
/// token is saved with `save_token` only for a session that is kept.
///
/// `open` and `close` stand for the portal, so that tests need no D-Bus.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn open_session_for_area<S, OpenFut, CloseFut>(
    restore_token: Option<String>,
    area: &MarkedArea,
    monitors: &[MonitorInfo],
    open: impl Fn(Option<String>) -> OpenFut,
    close: impl Fn(S) -> CloseFut,
    forget_token: impl Fn(),
    notice: impl Fn(String),
    withdraw_notice: impl Fn(),
    save_token: impl Fn(Option<String>),
) -> Result<(S, Vec<Stream>)>
where
    OpenFut: Future<Output = Result<(S, Vec<Stream>, Option<String>)>>,
    CloseFut: Future<Output = ()>,
{
    let (mut session, mut streams, mut new_token) = open(restore_token).await?;

    if let StreamMatch::Other { .. } = match_stream(&streams, area, monitors) {
        tracing::debug!(
            ?streams,
            "The shared stream is not the area's monitor; asking again"
        );
        close(session).await;
        forget_token();
        notice(gettext_f(
            "Choose {connector} (whole screen) in the sharing dialog",
            &[("connector", &area.connector)],
        ));

        let reopened = open(None).await;
        withdraw_notice();
        (session, streams, new_token) = reopened?;
        if let StreamMatch::Other { shared } = match_stream(&streams, area, monitors) {
            close(session).await;
            return Err(WrongMonitor {
                shared,
                connector: area.connector.clone(),
                monitor: (area.monitor_width, area.monitor_height),
            }
            .into());
        }
    }

    save_token(new_token);
    Ok((session, streams))
}

/// Runs right before the capture begins: before the countdown or, without
/// one, before the pipeline plays. An error aborts the recording.
pub type BeforeCapture = Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<()>>>>>;

#[derive(Default)]
struct BeforeCaptureHook(Option<BeforeCapture>);

impl fmt::Debug for BeforeCaptureHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BeforeCaptureHook")
            .field(&self.0.is_some())
            .finish()
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
        pub(super) before_capture: RefCell<BeforeCaptureHook>,

        pub(super) timer: RefCell<Option<Timer>>,
        pub(super) session: RefCell<Option<Session>>,
        pub(super) duration_source_id: RefCell<Option<glib::SourceId>>,
        pub(super) pipeline: OnceCell<gst::Pipeline>,
        pub(super) bus_watch_guard: RefCell<Option<BusWatchGuard>>,
        pub(super) area_border: RefCell<Option<(BorderGeometry, gdk::Monitor)>>,
        /// The H.264 encoder of the pipeline, if it has one.
        pub(super) encoder: Cell<Option<H264Encoder>>,
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
                    Signal::builder("notice")
                        .param_types([String::static_type()])
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

    /// The border of the selected area and the monitor it goes on, if the
    /// recording captures a selection of a whole monitor.
    pub fn area_border(&self) -> Option<(BorderGeometry, gdk::Monitor)> {
        self.imp().area_border.borrow().clone()
    }

    /// Sets what runs right before the capture begins; the camera `PiP`
    /// unmirrors itself there (P13).
    pub fn set_before_capture(&self, f: BeforeCapture) {
        self.imp()
            .before_capture
            .replace(BeforeCaptureHook(Some(f)));
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
        let area = area_to_record(
            settings.capture_mode(),
            overlay::availability().is_ok(),
            || Application::get().area_store().selected_area(),
        );
        let config = CaptureConfig::from_settings(settings, area)?;

        imp.capture_warnings.set(config.warnings());
        self.notify_capture_warnings();

        let monitors = gdk::Display::default()
            .map(|display| areas::monitors::current(&display))
            .unwrap_or_default();
        if let Some(chosen) = &config.area
            && let Some(staleness) = chosen.area.staleness(&monitors)
        {
            return Err(StaleArea {
                name: chosen.name.clone(),
                reason: staleness.describe(&chosen.area.connector),
            }
            .into());
        }

        // Setup screencast session
        let cursor_mode = if config.show_pointer {
            CursorMode::EMBEDDED
        } else {
            CursorMode::HIDDEN
        };
        let restore_token = settings.screencast_restore_token();
        settings.reset_screencast_restore_token();
        let (screencast_session, streams, fd) = if let Some(chosen) = &config.area {
            // A whole monitor, the area's (P9).
            let ((session, fd), streams) = open_session_for_area(
                Some(restore_token),
                &chosen.area,
                &monitors,
                |restore_token| async move {
                    let (session, streams, restore_token, fd) = new_screencast_session(
                        cursor_mode,
                        SourceType::MONITOR,
                        false,
                        restore_token.as_deref(),
                        PersistMode::Persistent,
                        parent,
                    )
                    .await
                    .with_context(failed_to_start_help)?;
                    Ok(((session, fd), streams, restore_token))
                },
                |(session, _fd): (Session, OwnedFd)| async move {
                    if let Err(err) = session.close().await {
                        tracing::warn!("Failed to close screencast session: {:?}", err);
                    }
                },
                || settings.reset_screencast_restore_token(),
                |message| self.emit_notice(&message),
                || Application::get().withdraw_notification("recording-notice"),
                |restore_token| {
                    settings.set_screencast_restore_token(&restore_token.unwrap_or_default());
                },
            )
            .await?;
            (session, streams, fd)
        } else {
            let (session, streams, restore_token, fd) = new_screencast_session(
                cursor_mode,
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
            .with_context(failed_to_start_help)?;
            settings.set_screencast_restore_token(&restore_token.unwrap_or_default());
            (session, streams, fd)
        };

        imp.session.replace(Some(screencast_session));

        let file_path =
            new_recording_path(&config.saving_location, config.profile.file_extension());
        imp.file.set(gio::File::for_path(&file_path)).unwrap();

        // `fd` stays owned until the pipeline exists, so that an error before
        // closes it; `pipewiresrc` only borrows the number.
        let raw_fd = fd.as_raw_fd();

        let mut pipeline_builder = PipelineBuilder::new(
            file_path,
            config.framerate,
            config.profile.clone(),
            raw_fd,
            streams.clone(),
        );

        // Select area
        if let Some(chosen) = &config.area {
            let area = &chosen.area;
            pipeline_builder.area_crop(AreaCrop {
                x: area.x,
                y: area.y,
                width: area.width,
                height: area.height,
                expected_frame: (area.monitor_width, area.monitor_height),
            });
            imp.area_border
                .replace(area_border::for_area(area, &monitors));
        } else if config.capture_mode == CaptureMode::Selection {
            let data = AreaSelector::select(raw_fd, &streams, &Application::get().window()).await?;
            imp.area_border
                .replace(area_border::for_streams(&streams, compute_crop(&data)));
            pipeline_builder.select_area_data(data);
        }

        if config.profile.supports_audio() {
            pipeline_builder.record_desktop_audio(config.record_desktop_audio);
            pipeline_builder.record_microphone(config.record_microphone);
        }

        pipeline_builder
            .pause_gate(imp.pause_gate.clone())
            .pause_flash(config.pause_flash.then_some(config.pause_flash_color));

        // Probed with the size of the area, known once the crop is (ADR 0006).
        let probe_size = pipeline_builder.probe_size();
        if let Some(encoder) = h264_encoder_for(config.profile, config.encoder, |encoder| {
            encoder.works(probe_size)
        })
        .await?
        {
            tracing::info!(?encoder, ?probe_size, "Recording with this H.264 encoder");
            pipeline_builder.encoder(encoder);
            imp.encoder.set(Some(encoder));
        }

        // Build pipeline
        let pipeline = pipeline_builder.build().with_context(|| {
            ContextWithHelp::new(
                gettext("Failed to start recording"),
                gettext("A GStreamer plugin may not be installed."),
            )
        })?;
        imp.pipeline.set(pipeline.clone()).unwrap();
        // From here on, the pipeline uses it for the rest of the recording.
        let _ = fd.into_raw_fd();

        // After the portal and the area selector, before the countdown.
        let before_capture = imp.before_capture.borrow().0.as_ref().map(|f| f());
        if let Some(before_capture) = before_capture {
            before_capture
                .await
                .context(gettext("The camera overlay did not refresh in time"))?;
        }

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

    /// Something the user must know while the recording starts, as a toast.
    pub fn connect_notice<F>(&self, f: F) -> glib::SignalHandlerId
    where
        F: Fn(&Self, &str) + 'static,
    {
        self.connect_closure(
            "notice",
            false,
            closure_local!(|obj: &Self, message: String| {
                f(obj, &message);
            }),
        )
    }

    fn emit_notice(&self, message: &str) {
        self.emit_by_name::<()>("notice", &[&message]);
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

                let error = if let Some(help) = encoder_error_help(imp.encoder.get(), e.src()) {
                    error.context(help)
                } else if e.error().matches(gst::ResourceError::OpenWrite) {
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

/// Help for a screencast session that failed to start.
fn failed_to_start_help() -> ContextWithHelp {
    ContextWithHelp::new(
        gettext("Failed to start recording"),
        gettext_f(
            // Translators: Do NOT translate the contents between '{' and '}', this is a variable name.
            "Check out {link} for help.",
            &[(
                "link",
                r#"<a href="https://github.com/SeaDve/Kooha#-it-doesnt-work">It Doesn't Work page</a>"#,
            )],
        ),
    )
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

    use crate::{
        areas::model::test_monitors::{dp3, edp, hdmi},
        settings::test_utils::{init_resources, settings_lock, setup_schema, temp_saving_location},
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

        let config = CaptureConfig::from_settings(&settings, None).unwrap();
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
    fn config_reads_the_encoder() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        settings.reset("profile-id");
        temp_saving_location(&settings);
        settings.set_video_encoder(VideoEncoder::Nvidia);

        let config = CaptureConfig::from_settings(&settings, None).unwrap();
        settings.set_video_encoder(VideoEncoder::Cpu);
        assert_eq!(config.encoder, VideoEncoder::Nvidia);

        settings.reset("saving-location");
        settings.reset("video-encoder");
    }

    /// Picks the encoder for the profile `id` with a fake probe where only
    /// `working` work, and returns the result and the encoders tried.
    fn encoder_with(
        id: &str,
        pref: VideoEncoder,
        working: &[H264Encoder],
    ) -> (Result<Option<H264Encoder>>, Vec<H264Encoder>) {
        init_resources();

        let tried = RefCell::new(Vec::new());
        let result = block_on(h264_encoder_for(
            Profile::from_id(id).unwrap(),
            pref,
            |encoder| {
                tried.borrow_mut().push(encoder);
                std::future::ready(if working.contains(&encoder) {
                    Ok(())
                } else {
                    Err(format!("{encoder:?} has no device"))
                })
            },
        ));
        (result, tried.into_inner())
    }

    #[test]
    fn webm_and_gif_need_no_h264_encoder() {
        for id in ["webm-vp8", "gif"] {
            let (result, tried) = encoder_with(id, VideoEncoder::Intel, &[]);
            assert_eq!(result.unwrap(), None, "{id}");
            assert!(tried.is_empty(), "{id}");
        }
    }

    #[test]
    fn h264_profiles_resolve_the_encoder() {
        for id in ["mp4", "matroska-h264"] {
            let (result, tried) = encoder_with(id, VideoEncoder::Auto, &[H264Encoder::Cpu]);
            assert_eq!(result.unwrap(), Some(H264Encoder::Cpu), "{id}");
            assert_eq!(tried, crate::encoder::AUTO_ORDER, "{id}");
        }
    }

    #[test]
    fn a_forced_encoder_that_fails_explains_how_to_fix_it() {
        let (result, tried) = encoder_with("mp4", VideoEncoder::Intel, &[H264Encoder::Cpu]);
        assert_eq!(tried, [H264Encoder::Intel]);

        let err = result.unwrap_err();
        assert_eq!(err.to_string(), "The Intel encoder is not working");
        assert_eq!(
            err.downcast_ref::<ContextWithHelp>()
                .unwrap()
                .help_message(),
            "Choose Automatic or CPU in Preferences."
        );
        assert!(format!("{err:?}").contains("Intel has no device"));
    }

    fn element(factory: &str) -> gst::Object {
        gst::init().unwrap();
        gst::ElementFactory::make(factory).build().unwrap().upcast()
    }

    #[test]
    fn an_encoder_error_while_recording_explains_how_to_fix_it() {
        let help = encoder_error_help(Some(H264Encoder::Cpu), Some(&element("x264enc"))).unwrap();
        assert_eq!(help.to_string(), "The CPU encoder is not working");
        assert_eq!(
            help.help_message(),
            "Choose Automatic or CPU in Preferences."
        );
    }

    #[test]
    fn other_errors_while_recording_have_no_encoder_help() {
        // Not an element of the chosen encoder's chain.
        assert!(encoder_error_help(Some(H264Encoder::Intel), Some(&element("x264enc"))).is_none());
        assert!(encoder_error_help(Some(H264Encoder::Cpu), Some(&element("filesink"))).is_none());
        // A converter or a caps filter of the chain, also elsewhere in the
        // pipeline: the selector, `videorate` or the audio mixer (fable code
        // r2 #3).
        assert!(encoder_error_help(Some(H264Encoder::Cpu), Some(&element("capsfilter"))).is_none());
        assert!(
            encoder_error_help(Some(H264Encoder::Cpu), Some(&element("videoconvert"))).is_none()
        );
        assert!(
            encoder_error_help(Some(H264Encoder::Intel), Some(&element("videoconvert"))).is_none()
        );
        // A profile with no H.264 encoder, or no source.
        assert!(encoder_error_help(None, Some(&element("x264enc"))).is_none());
        assert!(encoder_error_help(Some(H264Encoder::Cpu), None).is_none());
    }

    fn area_on(connector: &str, monitor: (i32, i32), scale: f64) -> MarkedArea {
        MarkedArea {
            connector: connector.into(),
            x: 100,
            y: 80,
            width: 640,
            height: 480,
            monitor_width: monitor.0,
            monitor_height: monitor.1,
            scale,
        }
    }

    fn on_dp3() -> MarkedArea {
        area_on("DP-3", (1680, 1050), 1.05)
    }

    fn on_edp() -> MarkedArea {
        area_on("eDP-1", (2560, 1600), 1.75)
    }

    fn monitors() -> Vec<MonitorInfo> {
        vec![hdmi(), edp(), dp3()]
    }

    /// A stream as KDE's portal sends it: a size and no position.
    fn stream(node_id: u32, size: Option<(i32, i32)>) -> Stream {
        let size = size.map_or_else(String::new, |(width, height)| {
            format!(", 'size': <({width}, {height})>")
        });
        let text = format!("(uint32 {node_id}, {{'source_type': <uint32 1>{size}}})");
        glib::Variant::parse(Some(&Stream::static_variant_type()), &text)
            .unwrap()
            .get::<Stream>()
            .unwrap()
    }

    #[test]
    fn marking_before_recording_only_in_selection_with_layers_and_no_area() {
        for mode in [CaptureMode::MonitorWindow, CaptureMode::Selection] {
            for overlays in [false, true] {
                for has_area in [false, true] {
                    let expected = mode == CaptureMode::Selection && overlays && !has_area;
                    assert_eq!(
                        needs_marking(mode, overlays, has_area),
                        expected,
                        "{mode:?}, overlays: {overlays}, has_area: {has_area}"
                    );
                }
            }
        }
    }

    #[test]
    fn area_to_record_only_in_selection_with_layers() {
        let chosen = || {
            Some(ChosenArea {
                name: Some("Demo".into()),
                area: on_dp3(),
            })
        };
        assert_eq!(
            area_to_record(CaptureMode::Selection, true, chosen),
            chosen()
        );
        assert_eq!(area_to_record(CaptureMode::Selection, false, chosen), None);
        assert_eq!(
            area_to_record(CaptureMode::MonitorWindow, true, chosen),
            None
        );
        assert_eq!(
            area_to_record(CaptureMode::MonitorWindow, false, chosen),
            None
        );
    }

    #[test]
    fn config_without_layers_has_no_area() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        settings.reset("profile-id");
        let chosen = || {
            Some(ChosenArea {
                name: None,
                area: on_dp3(),
            })
        };

        let config = CaptureConfig::from_settings(
            &settings,
            area_to_record(CaptureMode::Selection, false, chosen),
        )
        .unwrap();
        assert_eq!(config.area, None);

        let config = CaptureConfig::from_settings(
            &settings,
            area_to_record(CaptureMode::Selection, true, chosen),
        )
        .unwrap();
        assert_eq!(config.area, chosen());
    }

    #[test]
    fn stale_area_message() {
        let named = StaleArea {
            name: Some("Demo".into()),
            reason: "DP-3 is not connected".into(),
        };
        assert_eq!(
            named.message(),
            "The area “Demo” is out of date: DP-3 is not connected"
        );
        let last = StaleArea {
            name: None,
            reason: "DP-3 is not connected".into(),
        };
        assert_eq!(
            last.message(),
            "The last area is out of date: DP-3 is not connected"
        );
    }

    #[test]
    fn wrong_monitor_message() {
        let wrong = WrongMonitor {
            shared: Some((1920, 1080)),
            connector: "DP-3".into(),
            monitor: (1680, 1050),
        };
        assert_eq!(
            wrong.to_string(),
            "You shared 1920×1080, but the area is on DP-3 (1680×1050)"
        );
        let several = WrongMonitor {
            shared: None,
            ..wrong
        };
        assert_eq!(
            several.to_string(),
            "You shared more than one screen, but the area is on DP-3 (1680×1050)"
        );
    }

    #[test]
    fn stream_of_the_area_monitor_matches_by_size() {
        let monitors = monitors();
        // Physical, as KDE sends it.
        assert_eq!(
            match_stream(&[stream(1, Some((1680, 1050)))], &on_dp3(), &monitors),
            StreamMatch::Same
        );
        // Logical, as the portal spec says.
        assert_eq!(
            match_stream(&[stream(1, Some((1600, 1000)))], &on_dp3(), &monitors),
            StreamMatch::Same
        );
        // GDK says 1463×914 for the panel; the portal may round to 915.
        assert_eq!(
            match_stream(&[stream(1, Some((1463, 915)))], &on_edp(), &monitors),
            StreamMatch::Same
        );
    }

    #[test]
    fn stream_of_another_monitor_does_not_match() {
        let monitors = monitors();
        assert_eq!(
            match_stream(&[stream(1, Some((1920, 1080)))], &on_dp3(), &monitors),
            StreamMatch::Other {
                shared: Some((1920, 1080))
            }
        );
        // The whole desktop.
        assert_eq!(
            match_stream(&[stream(1, Some((3520, 2130)))], &on_dp3(), &monitors),
            StreamMatch::Other {
                shared: Some((3520, 2130))
            }
        );
        assert_eq!(
            match_stream(
                &[stream(1, Some((1680, 1050))), stream(2, Some((1920, 1080)))],
                &on_dp3(),
                &monitors
            ),
            StreamMatch::Other { shared: None }
        );
    }

    #[test]
    fn stream_without_size_is_unknown() {
        assert_eq!(
            match_stream(&[stream(1, None)], &on_dp3(), &monitors()),
            StreamMatch::Unknown
        );
    }

    /// What the portal stand-ins saw, in order.
    type Events = Rc<RefCell<Vec<String>>>;

    /// Opens sessions 1, 2, … whose stream has the next of `sizes`, with
    /// the restore token `t1`, `t2`, …
    async fn open_with(
        sizes: Vec<Option<(i32, i32)>>,
        events: &Events,
    ) -> Result<(u32, Vec<Stream>)> {
        let opened = Cell::new(0_u32);
        open_session_for_area(
            Some("saved".to_string()),
            &on_dp3(),
            &monitors(),
            |token| {
                let n = opened.get() + 1;
                opened.set(n);
                events.borrow_mut().push(format!("open {token:?}"));
                let size = sizes[n as usize - 1];
                async move { Ok((n, vec![stream(n, size)], Some(format!("t{n}")))) }
            },
            |session: u32| {
                events.borrow_mut().push(format!("close {session}"));
                async {}
            },
            || events.borrow_mut().push("forget".to_string()),
            |message| events.borrow_mut().push(format!("notice {message}")),
            || events.borrow_mut().push("withdraw notice".to_string()),
            |token| events.borrow_mut().push(format!("save {token:?}")),
        )
        .await
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        glib::MainContext::new().block_on(future)
    }

    #[test]
    fn same_monitor_at_once_keeps_the_session() {
        let events = Events::default();
        let (session, _) = block_on(open_with(vec![Some((1680, 1050))], &events)).unwrap();

        assert_eq!(session, 1);
        assert_eq!(
            *events.borrow(),
            [r#"open Some("saved")"#, r#"save Some("t1")"#]
        );
    }

    #[test]
    fn another_monitor_asks_again_without_the_token() {
        let events = Events::default();
        let (session, _) = block_on(open_with(
            vec![Some((1920, 1080)), Some((1680, 1050))],
            &events,
        ))
        .unwrap();

        assert_eq!(session, 2);
        assert_eq!(
            *events.borrow(),
            [
                r#"open Some("saved")"#,
                "close 1",
                "forget",
                "notice Choose DP-3 (whole screen) in the sharing dialog",
                "open None",
                // Before the recording starts: it could show in the video
                // (fable code r2 #2).
                "withdraw notice",
                r#"save Some("t2")"#,
            ]
        );
    }

    #[test]
    fn another_monitor_twice_fails_and_keeps_no_token() {
        let events = Events::default();
        let err = block_on(open_with(
            vec![Some((1920, 1080)), Some((2560, 1600))],
            &events,
        ))
        .unwrap_err();

        assert_eq!(
            err.downcast_ref::<WrongMonitor>(),
            Some(&WrongMonitor {
                shared: Some((2560, 1600)),
                connector: "DP-3".into(),
                monitor: (1680, 1050),
            })
        );
        assert_eq!(
            *events.borrow(),
            [
                r#"open Some("saved")"#,
                "close 1",
                "forget",
                "notice Choose DP-3 (whole screen) in the sharing dialog",
                "open None",
                "withdraw notice",
                "close 2",
            ]
        );
    }

    #[test]
    fn stream_without_size_is_recorded() {
        let events = Events::default();
        let (session, _) = block_on(open_with(vec![None], &events)).unwrap();

        assert_eq!(session, 1);
        assert_eq!(
            *events.borrow(),
            [r#"open Some("saved")"#, r#"save Some("t1")"#]
        );
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
