mod progress_icon;
mod toggle_button;

use adw::{prelude::*, subclass::prelude::*};
use anyhow::{Error, Result};
use gettextrs::gettext;
use gtk::{
    gio,
    glib::{self, clone},
};

use std::cell::{Cell, RefCell};

use self::{progress_icon::ProgressIcon, toggle_button::ToggleButton};
use crate::{
    Application,
    area_border::AreaBorder,
    camera_pip::{self, devices::CameraSource},
    cancelled::Cancelled,
    capture_warnings::{CaptureWarnings, WarningsNotifier},
    config::PROFILE,
    format,
    help::ContextWithHelp,
    overlay,
    preferences_dialog::PreferencesDialog,
    recording::{NoProfileError, Recording, RecordingState},
    settings::CaptureMode,
};

/// What the tray shows of the window's recording: its state, its duration
/// and its capture warnings.
pub type RecordingSnapshot = (RecordingState, gst::ClockTime, CaptureWarnings);

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/seadve/Kooha/ui/window.ui")]
    pub struct Window {
        #[template_child]
        pub(super) title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) main_page: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub(super) camera_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub(super) forget_video_sources_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub(super) recording_page: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) recording_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) recording_time_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) recording_warnings_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub(super) pause_record_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) delay_page: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) delay_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) delay_warnings_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub(super) flushing_page: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) flushing_progress_icon: TemplateChild<ProgressIcon>,

        pub(super) inhibit_cookie: RefCell<Option<u32>>,
        pub(super) warnings_notifier: WarningsNotifier,
        pub(super) recording: RefCell<Option<(Recording, Vec<glib::SignalHandlerId>)>>,
        /// The last whole second of the recording duration sent to the tray.
        pub(super) tray_duration_secs: Cell<u64>,
        /// The border of the recorded area, while it is on screen.
        pub(super) area_border: RefCell<Option<AreaBorder>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "KoohaWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            ToggleButton::ensure_type();

            klass.bind_template();

            klass.install_action_async("win.toggle-record", None, |obj, _, _| async move {
                obj.toggle_record().await;
            });

            klass.install_action("win.toggle-pause", None, move |obj, _, _| {
                if let Err(err) = obj.toggle_pause() {
                    let err = err.context(gettext("Failed to toggle pause"));
                    tracing::error!("{:?}", err);
                    obj.present_recording_error_dialog(&err);
                }
            });

            klass.install_action("win.cancel-record", None, move |obj, _, _| {
                obj.cancel_record();
            });

            klass.install_action("win.forget-video-sources", None, move |_obj, _, _| {
                Application::get()
                    .settings()
                    .reset_screencast_restore_token();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            if PROFILE == "Devel" {
                obj.add_css_class("devel");
            }

            obj.setup_settings();
            obj.setup_camera_button();

            obj.update_view();
            obj.update_title_label();
            obj.update_subtitle_label();
            obj.update_forget_video_sources_action();
        }
    }

    impl WidgetImpl for Window {
        fn realize(&self) {
            self.parent_realize();

            // On Wayland the compositor tells the monitor only after the
            // first commit, so asking on `map` would give the wrong one
            // (fable r3 #1).
            if let Some(surface) = self.obj().surface() {
                surface.connect_enter_monitor(|_, monitor| {
                    if let Some(camera_pip) = Application::get().camera_pip() {
                        camera_pip.set_kooha_monitor(Some(monitor.clone()));
                    }
                });
            }
        }

        fn map(&self) {
            self.parent_map();

            let obj = self.obj();

            obj.update_audio_actions();
        }
    }

    impl WindowImpl for Window {
        fn close_request(&self) -> glib::Propagation {
            let obj = self.obj();

            if obj.is_busy() {
                glib::spawn_future_local(clone!(
                    #[weak]
                    obj,
                    async move {
                        if obj.run_quit_confirmation_dialog().await.is_proceed() {
                            obj.destroy();
                        }
                    }
                ));
                return glib::Propagation::Stop;
            }

            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gio::ActionMap, gtk::ShortcutManager, gio::ActionGroup, gtk::Native, gtk::Root;
}

impl Window {
    pub fn new(app: &Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    /// Returns `true` if the window is busy with a recording.
    pub fn is_busy(&self) -> bool {
        self.imp()
            .recording
            .borrow()
            .as_ref()
            .is_some_and(|(recording, _)| {
                matches!(
                    recording.state(),
                    RecordingState::Recording
                        | RecordingState::Paused
                        | RecordingState::Flushing { .. }
                )
            })
    }

    /// Returns the state of the recording while there is one, also before it
    /// begins.
    pub fn recording_snapshot(&self) -> Option<RecordingSnapshot> {
        self.imp()
            .recording
            .borrow()
            .as_ref()
            .map(|(recording, _)| {
                (
                    recording.state(),
                    recording.duration(),
                    recording.capture_warnings(),
                )
            })
    }

    fn update_tray(&self) {
        Application::get().update_tray(self.recording_snapshot());
    }

    /// Returns `Proceed` if the user wants to proceed with the quit operation.
    pub async fn run_quit_confirmation_dialog(&self) -> glib::Propagation {
        const CANCEL_RESPONSE_ID: &str = "cancel";
        const QUIT_RESPONSE_ID: &str = "quit";

        debug_assert!(
            self.is_busy(),
            "quit confirmation dialog must only be presented when busy"
        );

        let body_text = match self
            .imp()
            .recording
            .borrow()
            .as_ref()
            .map(|(recording, _)| recording.state())
        {
            Some(RecordingState::Recording) | Some(RecordingState::Paused) => gettext(
                "A recording is currently in progress. Quitting immediately may cause the recording to be unplayable. Please stop the recording before quitting.",
            ),
            Some(RecordingState::Flushing { .. }) => gettext(
                "Quitting will cancel the processing and may cause the recording to be unplayable.",
            ),
            state => unreachable!("unexpected recording state: {:?}", state),
        };

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Quit the Application?"))
            .body(body_text)
            .close_response(CANCEL_RESPONSE_ID)
            .default_response(CANCEL_RESPONSE_ID)
            .build();
        dialog.add_response(CANCEL_RESPONSE_ID, &gettext("Cancel"));

        dialog.add_response(QUIT_RESPONSE_ID, &gettext("Quit"));
        dialog.set_response_appearance(QUIT_RESPONSE_ID, adw::ResponseAppearance::Destructive);

        match dialog.choose_future(Some(self)).await.as_str() {
            CANCEL_RESPONSE_ID => glib::Propagation::Stop,
            QUIT_RESPONSE_ID => glib::Propagation::Proceed,
            _ => unreachable!(),
        }
    }

    fn present_recording_error_dialog(&self, err: &Error) {
        const OK_RESPONSE_ID: &str = "ok";

        let err_text = format!("{:?}", err);

        let err_view = gtk::TextView::builder()
            .buffer(&gtk::TextBuffer::builder().text(&err_text).build())
            .editable(false)
            .monospace(true)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();

        let scrolled_window = gtk::ScrolledWindow::builder()
            .min_content_width(360)
            .min_content_height(120)
            .child(&err_view)
            .build();

        let scrolled_window_row = gtk::ListBoxRow::builder()
            .overflow(gtk::Overflow::Hidden)
            .activatable(false)
            .selectable(false)
            .child(&scrolled_window)
            .build();
        scrolled_window_row.add_css_class("error-view");

        let copy_button = gtk::Button::builder()
            .valign(gtk::Align::Center)
            .tooltip_text(gettext("Copy to clipboard"))
            .icon_name("edit-copy-symbolic")
            .build();
        copy_button.connect_clicked(move |button| {
            button.clipboard().set_text(&err_text);
            button.set_tooltip_text(Some(&gettext("Copied to clipboard")));
            button.set_icon_name("checkmark-symbolic");
            button.add_css_class("copy-done");
        });

        let expander = adw::ExpanderRow::builder()
            .title(gettext("Show detailed error"))
            .activatable(false)
            .build();
        expander.add_row(&scrolled_window_row);
        expander.add_suffix(&copy_button);

        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list_box.add_css_class("boxed-list");
        list_box.append(&expander);

        let dialog = adw::AlertDialog::builder()
            .heading(err.to_string())
            .body_use_markup(true)
            .default_response(OK_RESPONSE_ID)
            .extra_child(&list_box)
            .build();

        if let Some(context) = err.downcast_ref::<ContextWithHelp>() {
            dialog.set_body(&format!(
                "<b>{}</b>: {}",
                gettext("Help"),
                context.help_message()
            ));
        }

        dialog.add_response(OK_RESPONSE_ID, &gettext("Ok, Got It"));
        dialog.present(Some(self));
    }

    fn present_no_profile_error_dialog(&self) {
        const OPEN_RESPONSE_ID: &str = "open";
        const LATER_RESPONSE_ID: &str = "later";

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Open Preferences?"))
            .body(gettext("The previously selected format may have been unavailable. Open preferences and select a format to continue recording."))
            .default_response(OPEN_RESPONSE_ID)
            .build();

        dialog.add_response(LATER_RESPONSE_ID, &gettext("Later"));

        dialog.add_response(OPEN_RESPONSE_ID, &gettext("Open"));
        dialog.set_response_appearance(OPEN_RESPONSE_ID, adw::ResponseAppearance::Suggested);

        dialog.connect_response(
            Some(OPEN_RESPONSE_ID),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |dialog, _| {
                    dialog.close();

                    let app = Application::get();
                    let preferences_dialog = PreferencesDialog::new(app.settings());
                    preferences_dialog.present(Some(&obj));

                    let was_focused = preferences_dialog.profile_row_grab_focus();
                    debug_assert!(was_focused);
                }
            ),
        );

        dialog.present(Some(self));
    }

    async fn toggle_record(&self) {
        let imp = self.imp();

        if let Some((ref recording, _)) = *imp.recording.borrow() {
            recording.stop();
            return;
        }

        let recording = Recording::new();
        imp.warnings_notifier.reset();
        let handler_ids = vec![
            recording.connect_state_notify(clone!(
                #[weak(rename_to = obj)]
                self,
                move |recording| {
                    obj.update_view();
                    obj.update_inhibit();

                    obj.imp().warnings_notifier.on_state(
                        recording.state(),
                        recording.capture_warnings(),
                        |warnings| {
                            Application::get().send_capture_warnings_notification(warnings);
                        },
                    );
                }
            )),
            recording.connect_capture_warnings_notify(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.update_warnings_boxes();
                    obj.update_tray();
                }
            )),
            recording.connect_duration_notify(clone!(
                #[weak(rename_to = obj)]
                self,
                move |recording| {
                    let imp = obj.imp();

                    let duration = recording.duration();
                    let formatted_time = format::digital_clock(duration);
                    imp.recording_time_label.set_label(&formatted_time);

                    // The duration changes several times per second, but the
                    // tray only shows whole seconds.
                    if imp.tray_duration_secs.replace(duration.seconds()) != duration.seconds() {
                        obj.update_tray();
                    }
                }
            )),
            recording.connect_finished(clone!(
                #[weak(rename_to = obj)]
                self,
                move |recording, res| {
                    obj.handle_recording_finished(recording, res);
                }
            )),
        ];
        imp.recording
            .replace(Some((recording.clone(), handler_ids)));
        imp.tray_duration_secs.set(0);

        // Set after storing the recording, so a second Ctrl+R meanwhile stops it.
        if let Some(camera_pip) = Application::get().camera_pip() {
            let camera_pip = camera_pip.clone();
            recording.set_before_capture(Box::new(move || {
                let camera_pip = camera_pip.clone();
                Box::pin(async move {
                    camera_pip.set_recording_active(true);
                    camera_pip.prepare_for_recording().await
                })
            }));
        }

        // Show `Starting` while the portal dialog is open.
        self.update_tray();
        self.update_inhibit();

        recording
            .start(Some(self), Application::get().settings())
            .await;
    }

    fn toggle_pause(&self) -> Result<()> {
        let imp = self.imp();

        if let Some((ref recording, _)) = *imp.recording.borrow() {
            if matches!(recording.state(), RecordingState::Paused) {
                recording.resume()?;
            } else {
                recording.pause()?;
            };
        }

        Ok(())
    }

    fn cancel_record(&self) {
        let imp = self.imp();

        if let Some((ref recording, _)) = *imp.recording.borrow() {
            recording.cancel();
        }
    }

    fn handle_recording_finished(
        &self,
        recording: &Recording,
        res: &Result<(gio::File, gst::ClockTime)>,
    ) {
        debug_assert_eq!(recording.state(), RecordingState::Finished);

        let app = Application::get();
        app.withdraw_notification("capture-warnings");
        if let Some(camera_pip) = app.camera_pip() {
            camera_pip.set_recording_active(false);
        }

        match res {
            Ok((recording_file, duration)) => {
                let duration = *duration;
                glib::spawn_future_local(clone!(
                    #[strong]
                    recording_file,
                    async move {
                        let app = Application::get();
                        app.send_record_success_notification(&recording_file, duration)
                            .await;
                    }
                ));

                let recent_manager = gtk::RecentManager::default();
                recent_manager.add_item(&recording_file.uri());
            }
            Err(err) => {
                if err.is::<Cancelled>() {
                    tracing::debug!("{:?}", err);
                } else if err.is::<NoProfileError>() {
                    self.present_no_profile_error_dialog();
                } else {
                    tracing::error!("{:?}", err);

                    self.present_recording_error_dialog(err);

                    if let Some(surface) = self.surface() {
                        surface.beep();
                    }
                }
            }
        }

        if let Some((recording, handler_ids)) = self.imp().recording.take() {
            for handler_id in handler_ids {
                recording.disconnect(handler_id);
            }

            self.update_tray();
            self.update_inhibit();
        } else {
            tracing::warn!("Recording finished but no stored recording");
        }
    }

    fn update_inhibit(&self) {
        let imp = self.imp();

        let app = Application::get();
        let is_busy = self.is_busy();

        if is_busy && imp.inhibit_cookie.borrow().is_none() {
            let inhibit_cookie = app.inhibit(
                Some(self),
                gtk::ApplicationInhibitFlags::LOGOUT | gtk::ApplicationInhibitFlags::IDLE,
                Some(&gettext("A recording is in progress")),
            );
            imp.inhibit_cookie.replace(Some(inhibit_cookie));

            tracing::debug!("Inhibited logout and idle");
        } else if !is_busy && let Some(inhibit_cookie) = imp.inhibit_cookie.take() {
            app.uninhibit(inhibit_cookie);

            tracing::debug!("Uninhibited logout and idle");
        }
    }

    fn update_view(&self) {
        let imp = self.imp();

        // TODO disregard ms granularity recording state change

        let state = imp
            .recording
            .borrow()
            .as_ref()
            .map_or(RecordingState::Init, |(recording, _)| recording.state());

        match state {
            RecordingState::Init | RecordingState::Finished => {
                imp.stack.set_visible_child(&*imp.main_page);

                imp.recording_time_label
                    .set_label(&format::digital_clock(gst::ClockTime::ZERO));
            }
            RecordingState::Delayed { secs_left } => {
                imp.delay_label.set_label(&secs_left.to_string());

                imp.stack.set_visible_child(&*imp.delay_page);
            }
            RecordingState::Recording => {
                imp.pause_record_button
                    .set_icon_name("media-playback-pause-symbolic");
                imp.recording_label.set_label(&gettext("Recording"));
                imp.recording_time_label.remove_css_class("paused");

                imp.stack.set_visible_child(&*imp.recording_page);
            }
            RecordingState::Paused => {
                imp.pause_record_button
                    .set_icon_name("media-playback-start-symbolic");
                imp.recording_label.set_label(&gettext("Paused"));
                imp.recording_time_label.add_css_class("paused");

                imp.stack.set_visible_child(&*imp.recording_page);
            }
            RecordingState::Flushing { progress } => {
                imp.flushing_progress_icon
                    .set_progress(progress as f64 / 100.0);

                imp.stack.set_visible_child(&*imp.flushing_page);
            }
        }

        self.update_warnings_boxes();
        self.update_tray();
        self.update_area_border(state);

        self.action_set_enabled(
            "win.toggle-record",
            !matches!(
                state,
                RecordingState::Delayed { .. } | RecordingState::Flushing { .. }
            ),
        );
        self.action_set_enabled(
            "win.toggle-pause",
            matches!(state, RecordingState::Recording | RecordingState::Paused),
        );
        self.action_set_enabled(
            "win.cancel-record",
            matches!(
                state,
                RecordingState::Delayed { .. } | RecordingState::Flushing { .. }
            ),
        );
    }

    /// Shows the border of the recorded area during the countdown, the
    /// recording and the pause (P14), once per recording; removes it in any
    /// other state.
    fn update_area_border(&self, state: RecordingState) {
        let imp = self.imp();

        if !shows_area_border(state) {
            // Dropping it destroys the layer.
            imp.area_border.take();
            return;
        }

        if imp.area_border.borrow().is_some() {
            return;
        }

        let Some((border, monitor)) = imp
            .recording
            .borrow()
            .as_ref()
            .and_then(|(recording, _)| recording.area_border())
        else {
            return;
        };

        if let Err(reason) = overlay::availability() {
            tracing::debug!(?reason, "No area border: overlays are unavailable");
            return;
        }

        let area_border = AreaBorder::new(border, &monitor);
        area_border.present();
        imp.area_border.replace(Some(area_border));
    }

    fn update_warnings_boxes(&self) {
        let imp = self.imp();

        let warnings = imp
            .recording
            .borrow()
            .as_ref()
            .map(|(recording, _)| recording.capture_warnings())
            .unwrap_or_default();

        for flow_box in [&*imp.recording_warnings_box, &*imp.delay_warnings_box] {
            flow_box.remove_all();
            for chip in warning_chips(warnings) {
                flow_box.append(&chip);
            }
            flow_box.set_visible(!warnings.is_empty());
        }
    }

    fn update_title_label(&self) {
        let imp = self.imp();

        match Application::get().settings().capture_mode() {
            CaptureMode::MonitorWindow => imp.title.set_title(&gettext("Normal")),
            CaptureMode::Selection => imp.title.set_title(&gettext("Selection")),
        }
    }

    fn update_subtitle_label(&self) {
        let imp = self.imp();

        let app = Application::get();
        let settings = app.settings();

        let profile_text = settings
            .profile()
            .map_or_else(|| gettext("None"), |profile| profile.name().to_string());
        let framerate_text = format::framerate(settings.framerate());

        imp.title
            .set_subtitle(&format!("{} • {} FPS", profile_text, framerate_text));
    }

    fn update_audio_actions(&self) {
        let is_enabled = Application::get()
            .settings()
            .profile()
            .is_none_or(|profile| profile.supports_audio());

        self.action_set_enabled("win.record-desktop-audio", is_enabled);
        self.action_set_enabled("win.record-microphone", is_enabled);
    }

    fn update_forget_video_sources_action(&self) {
        let has_restore_token = !Application::get()
            .settings()
            .screencast_restore_token()
            .is_empty();

        self.imp()
            .forget_video_sources_revealer
            .set_reveal_child(has_restore_token);

        self.action_set_enabled("win.forget-video-sources", has_restore_token);
    }

    fn setup_settings(&self) {
        let app = Application::get();
        let settings = app.settings();

        settings.connect_capture_mode_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.update_title_label();
            }
        ));

        settings.connect_profile_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.update_audio_actions();
                obj.update_subtitle_label();
            }
        ));

        settings.connect_framerate_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.update_subtitle_label();
            }
        ));

        settings.connect_screencast_restore_token_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.update_forget_video_sources_action();
            }
        ));

        self.add_action(&settings.create_record_desktop_audio_action());
        self.add_action(&settings.create_record_microphone_action());
        self.add_action(&settings.create_show_pointer_action());
        self.add_action(&settings.create_capture_mode_action());
        self.add_action(&settings.create_camera_enabled_action());
        self.add_action(&settings.create_camera_device_action());
    }

    fn setup_camera_button(&self) {
        let camera_button = &self.imp().camera_button;

        if let Err(reason) = overlay::availability() {
            tracing::debug!(
                "Camera button disabled: overlays unavailable ({:?})",
                reason
            );
            camera_button.set_sensitive(false);
            camera_button.set_tooltip_text(Some(&gettext("Camera overlay unavailable")));
            return;
        }

        // Refreshes the sources every time the menu opens, from the device
        // monitor of the `CameraPip`.
        camera_button.set_create_popup_func(|button| {
            button.set_menu_model(Some(&camera_menu(&camera_pip::app_sources())));
        });
    }
}

/// The camera button menu: the on/off toggle and one radio item per source.
fn camera_menu(sources: &[CameraSource]) -> gio::Menu {
    let toggle_section = gio::Menu::new();
    toggle_section.append(Some(&gettext("Show Camera")), Some("win.camera-enabled"));

    let sources_section = gio::Menu::new();
    for source in sources {
        let item = gio::MenuItem::new(Some(&source.name), None);
        item.set_action_and_target_value(Some("win.camera-device"), Some(&source.id.to_variant()));
        sources_section.append_item(&item);
    }

    let menu = gio::Menu::new();
    menu.append_section(None, &toggle_section);
    menu.append_section(Some(&gettext("Camera Source")), &sources_section);
    menu
}

/// Whether the border of the recorded area is on screen in `state`: during
/// the countdown, the recording and the pause (P14).
fn shows_area_border(state: RecordingState) -> bool {
    matches!(
        state,
        RecordingState::Delayed { .. } | RecordingState::Recording | RecordingState::Paused
    )
}

/// One label per capture warning, styled as a chip.
fn warning_chips(warnings: CaptureWarnings) -> Vec<gtk::Label> {
    warnings
        .labels()
        .into_iter()
        .map(|label| {
            let chip = gtk::Label::new(Some(&format!("⚠ {label}")));
            chip.add_css_class("capture-warning");
            chip
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_pip::devices::CameraSource;

    #[gtk::test]
    fn warning_chips_have_one_label_per_warning() {
        let chips = warning_chips(CaptureWarnings::compute(true, true, false, false));

        let texts = chips.iter().map(|chip| chip.label()).collect::<Vec<_>>();
        assert_eq!(texts, ["⚠ Without microphone", "⚠ Without mouse pointer"]);
        assert!(
            chips
                .iter()
                .all(|chip| chip.has_css_class("capture-warning"))
        );
    }

    #[gtk::test]
    fn no_warning_chips_without_warnings() {
        assert!(warning_chips(CaptureWarnings::default()).is_empty());
    }

    #[test]
    fn area_border_shows_from_countdown_until_stop() {
        for state in [
            RecordingState::Delayed { secs_left: 3 },
            RecordingState::Recording,
            RecordingState::Paused,
        ] {
            assert!(shows_area_border(state), "{state:?}");
        }
        for state in [
            RecordingState::Init,
            RecordingState::Flushing { progress: 50 },
            RecordingState::Finished,
        ] {
            assert!(!shows_area_border(state), "{state:?}");
        }
    }

    #[gtk::test]
    fn pause_controls_are_shown() {
        let _lock = crate::settings::test_utils::settings_lock();
        crate::settings::test_utils::setup_schema();
        crate::settings::test_utils::init_resources();
        gst::init().unwrap();

        let window = Window::new(&Application::new());
        // The window is never presented, so this reads the button's own
        // `visible` and not whether it is on screen.
        assert!(window.imp().pause_record_button.get_visible());

        // The shortcuts dialog lists the pause shortcut.
        let builder = gtk::Builder::from_resource("/io/github/seadve/Kooha/shortcuts-dialog.ui");
        let lists_toggle_pause = builder.objects().iter().any(|object| {
            object.find_property("action-name").is_some()
                && object.property::<Option<String>>("action-name").as_deref()
                    == Some("win.toggle-pause")
        });
        assert!(lists_toggle_pause);
    }

    #[gtk::test]
    fn camera_button_follows_overlay_availability() {
        let _lock = crate::settings::test_utils::settings_lock();
        crate::settings::test_utils::setup_schema();
        crate::settings::test_utils::init_resources();
        gst::init().unwrap();

        let window = Window::new(&Application::new());
        let button = window.imp().camera_button.get();

        assert!(window.lookup_action("camera-enabled").is_some());
        assert!(window.lookup_action("camera-device").is_some());

        let available = crate::overlay::availability().is_ok();
        assert_eq!(button.is_sensitive(), available);
        if !available {
            assert_eq!(
                button.tooltip_text().as_deref(),
                Some("Camera overlay unavailable")
            );
        }
    }

    #[gtk::test]
    fn camera_menu_lists_sources() {
        let sources = [
            CameraSource {
                id: "/dev/video0".to_string(),
                name: "Webcam".to_string(),
            },
            CameraSource {
                id: "OBS".to_string(),
                name: "OBS Virtual Camera".to_string(),
            },
        ];

        let menu = camera_menu(&sources);
        let item = |section: i32, index: i32| {
            let section = menu
                .item_link(section, gio::MENU_LINK_SECTION)
                .expect("menu has the section");
            let label = section
                .item_attribute_value(index, gio::MENU_ATTRIBUTE_LABEL, None)
                .and_then(|value| value.get::<String>());
            let action = section
                .item_attribute_value(index, gio::MENU_ATTRIBUTE_ACTION, None)
                .and_then(|value| value.get::<String>());
            let target = section
                .item_attribute_value(index, gio::MENU_ATTRIBUTE_TARGET, None)
                .and_then(|value| value.get::<String>());
            (label, action, target)
        };

        assert_eq!(
            item(0, 0),
            (
                Some("Show Camera".to_string()),
                Some("win.camera-enabled".to_string()),
                None
            )
        );
        assert_eq!(
            item(1, 0),
            (
                Some("Webcam".to_string()),
                Some("win.camera-device".to_string()),
                Some("/dev/video0".to_string())
            )
        );
        assert_eq!(
            item(1, 1),
            (
                Some("OBS Virtual Camera".to_string()),
                Some("win.camera-device".to_string()),
                Some("OBS".to_string())
            )
        );
    }
}
