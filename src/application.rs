use adw::{prelude::*, subclass::prelude::*};
use futures_util::StreamExt;
use gettextrs::gettext;
use gtk::{
    gio,
    glib::{self, clone},
};

use crate::{
    about,
    camera_pip::CameraPip,
    capture_warnings::CaptureWarnings,
    config::{APP_ID, PKGDATADIR, PROFILE, VERSION},
    doctor::dialog::RequirementsDialog,
    format, overlay,
    preferences_dialog::PreferencesDialog,
    settings::Settings,
    tray::{TrayCommand, TrayModel, TrayRequest, TrayService, model::tray_state},
    window::{RecordingSnapshot, Window},
};

mod imp {
    use std::cell::{Cell, OnceCell};

    use super::*;

    #[derive(Debug, Default)]
    pub struct Application {
        pub(super) settings: OnceCell<Settings>,
        pub(super) tray: OnceCell<TrayService>,
        pub(super) camera_pip: OnceCell<CameraPip>,
        /// The last snapshot the window sent, reused on settings changes.
        pub(super) tray_snapshot: Cell<Option<RecordingSnapshot>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "KoohaApplication";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn activate(&self) {
            self.parent_activate();

            let obj = self.obj();

            if let Some(window) = obj.windows().first() {
                window.present();
                return;
            }

            let window = Window::new(&obj);
            window.present();
        }

        fn startup(&self) {
            self.parent_startup();

            gtk::Window::set_default_icon_name(APP_ID);

            let obj = self.obj();

            obj.setup_gactions();
            obj.setup_accels();
            obj.setup_tray();
            obj.setup_camera_pip();
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionMap, gio::ActionGroup;
}

impl Application {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("resource-base-path", "/io/github/seadve/Kooha/")
            .build()
    }

    /// Returns the global instance of `Application`.
    ///
    /// # Panics
    ///
    /// Panics if the app is not running or if this is called on a non-main thread.
    pub fn get() -> Self {
        debug_assert!(
            gtk::is_initialized_main_thread(),
            "application must only be accessed in the main thread"
        );

        gio::Application::default().unwrap().downcast().unwrap()
    }

    pub fn settings(&self) -> &Settings {
        self.imp().settings.get_or_init(|| {
            let settings = Settings::default();

            if tracing::enabled!(tracing::Level::TRACE) {
                settings.connect_changed(None, |settings, key| {
                    tracing::trace!("Settings `{}` changed to `{}`", key, settings.value(key));
                });
            }

            settings
        })
    }

    /// The camera `PiP`, created in `startup`.
    pub fn camera_pip(&self) -> Option<&CameraPip> {
        self.imp().camera_pip.get()
    }

    fn setup_camera_pip(&self) {
        // Without a window yet: the PiP starts mirrored, on the saved or the
        // first monitor, and the window tells it the rest.
        let camera_pip = CameraPip::new(self.settings());
        self.imp().camera_pip.set(camera_pip).unwrap();
    }

    pub fn window(&self) -> Window {
        self.active_window()
            .map_or_else(|| Window::new(self), |w| w.downcast().unwrap())
    }

    pub async fn send_record_success_notification(
        &self,
        recording_file: &gio::File,
        duration: gst::ClockTime,
    ) {
        let mut body_fragments = vec![format::duration(duration)];

        match recording_file
            .query_info_future(
                gio::FILE_ATTRIBUTE_STANDARD_SIZE,
                gio::FileQueryInfoFlags::NONE,
                glib::Priority::DEFAULT_IDLE,
            )
            .await
        {
            Ok(file_info) => {
                let formatted_size = glib::format_size(file_info.size() as u64);
                body_fragments.push(formatted_size.to_string());
            }
            Err(err) => tracing::warn!("Failed to get file size: {:?}", err),
        }

        // Translators: This is a message that the user will see when the recording is finished.
        let notification = gio::Notification::new(&gettext("Screencast recorded"));
        notification.set_body(Some(&body_fragments.join(", ")));
        notification.set_default_action_and_target_value(
            "app.launch-uri",
            Some(&recording_file.uri().to_variant()),
        );
        notification.add_button_with_target_value(
            &gettext("Show in Files"),
            "app.show-in-files",
            Some(&recording_file.uri().to_variant()),
        );

        self.send_notification(Some("record-success"), &notification);
    }

    pub fn send_capture_warnings_notification(&self, warnings: CaptureWarnings) {
        if warnings.is_empty() {
            return;
        }

        let notification = gio::Notification::new(&gettext("Recording started"));
        notification.set_body(Some(&warnings.labels().join(" · ")));

        self.send_notification(Some("capture-warnings"), &notification);
    }

    /// Shows `snapshot`, the window's recording, in the tray.
    ///
    /// This must never call `Application::window()`: the window calls this
    /// from its `constructed`, before the app lists it, so `window()` would
    /// build another window.
    pub fn update_tray(&self, snapshot: Option<RecordingSnapshot>) {
        let imp = self.imp();

        imp.tray_snapshot.set(snapshot);

        if let Some(tray) = imp.tray.get() {
            tray.update(tray_model(snapshot, self.settings()));
        }
    }

    fn refresh_tray(&self) {
        self.update_tray(self.imp().tray_snapshot.get());
    }

    fn setup_tray(&self) {
        let settings = self.settings();

        let (sender, mut receiver) = futures_channel::mpsc::unbounded();
        let tray = TrayService::spawn(sender, tray_model(None, settings));
        self.imp().tray.set(tray).unwrap();

        glib::spawn_future_local(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                while let Some(request) = receiver.next().await {
                    obj.handle_tray_command(request);
                }
            }
        ));

        settings.connect_record_desktop_audio_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.refresh_tray()
        ));
        settings.connect_record_microphone_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.refresh_tray()
        ));
        settings.connect_show_pointer_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.refresh_tray()
        ));
        settings.connect_profile_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.refresh_tray()
        ));
        settings.connect_camera_enabled_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.refresh_tray()
        ));
    }

    fn handle_tray_command(&self, request: TrayRequest) {
        let TrayRequest {
            command,
            activation_token,
        } = request;

        match command {
            TrayCommand::ToggleRecord => {
                if let Err(err) =
                    WidgetExt::activate_action(&self.window(), "win.toggle-record", None)
                {
                    tracing::warn!("Failed to toggle record from the tray: {:?}", err);
                }
            }
            TrayCommand::TogglePause => {
                if let Err(err) =
                    WidgetExt::activate_action(&self.window(), "win.toggle-pause", None)
                {
                    tracing::warn!("Failed to toggle pause from the tray: {:?}", err);
                }
            }
            TrayCommand::ToggleCamera => {
                let settings = self.settings();
                settings.set_camera_enabled(!settings.camera_enabled());
            }
            TrayCommand::ShowWindow => self.present_window(activation_token.as_deref()),
            TrayCommand::Quit => {
                // The quit confirmation needs the window on screen.
                self.present_window(activation_token.as_deref());
                ActionGroupExt::activate_action(self, "quit", None);
            }
            TrayCommand::ToggleDesktopAudio
            | TrayCommand::ToggleMicrophone
            | TrayCommand::TogglePointer => {
                let has_recording = self
                    .active_window()
                    .and_downcast::<Window>()
                    .is_some_and(|window| window.recording_snapshot().is_some());
                apply_tray_command(command, self.settings(), has_recording);
            }
        }
    }

    /// Brings the window to the front. On Wayland the compositor only does
    /// it with the activation token of the click (fable code r1 #4).
    fn present_window(&self, activation_token: Option<&str>) {
        let window = self.window();
        if let Some(token) = activation_token {
            window.set_startup_id(token);
        }
        window.present();
    }

    pub fn run(&self) -> glib::ExitCode {
        tracing::info!("Kooha ({})", APP_ID);
        tracing::info!("Version: {} ({})", VERSION, PROFILE);
        tracing::info!("Datadir: {}", PKGDATADIR);

        ApplicationExtManual::run(self)
    }

    pub fn quit(&self) {
        glib::spawn_future_local(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if obj.quit_request().await.is_proceed() {
                    ApplicationExt::quit(&obj);
                }
            }
        ));
    }

    async fn quit_request(&self) -> glib::Propagation {
        if let Some(window) = self.active_window() {
            let window = window.downcast::<Window>().unwrap();

            if window.is_busy() {
                return window.run_quit_confirmation_dialog().await;
            }
        }

        glib::Propagation::Proceed
    }

    fn setup_gactions(&self) {
        let launch_uri_action = gio::ActionEntry::builder("launch-uri")
            .parameter_type(Some(&String::static_variant_type()))
            .activate(|obj: &Self, _, param| {
                let uri = param.unwrap().get::<String>().unwrap();
                glib::spawn_future_local(clone!(
                    #[strong]
                    obj,
                    async move {
                        if let Err(err) = gtk::FileLauncher::new(Some(&gio::File::for_uri(&uri)))
                            .launch_future(obj.active_window().as_ref())
                            .await
                        {
                            tracing::error!("Failed to launch uri `{}`: {:?}", uri, err);
                        }
                    }
                ));
            })
            .build();
        let show_in_files_action = gio::ActionEntry::builder("show-in-files")
            .parameter_type(Some(&String::static_variant_type()))
            .activate(|obj: &Self, _, param| {
                let uri = param.unwrap().get::<String>().unwrap();
                glib::spawn_future_local(clone!(
                    #[strong]
                    obj,
                    async move {
                        if let Err(err) = gtk::FileLauncher::new(Some(&gio::File::for_uri(&uri)))
                            .open_containing_folder_future(obj.active_window().as_ref())
                            .await
                        {
                            tracing::warn!("Failed to show `{}` in files: {:?}", uri, err);
                        }
                    }
                ));
            })
            .build();
        let quit_action = gio::ActionEntry::builder("quit")
            .activate(|obj: &Self, _, _| {
                obj.quit();
            })
            .build();
        let show_preferences_action = gio::ActionEntry::builder("show-preferences")
            .activate(|obj: &Self, _, _| {
                let dialog = PreferencesDialog::new(obj.settings());
                dialog.present(Some(&obj.window()));
            })
            .build();
        let show_requirements_report_action = gio::ActionEntry::builder("show-requirements-report")
            .activate(|obj: &Self, _, _| {
                RequirementsDialog::present_and_run(&obj.window(), obj.settings());
            })
            .build();
        let show_about_action = gio::ActionEntry::builder("show-about")
            .activate(|obj: &Self, _, _| {
                about::present_dialog(&obj.window());
            })
            .build();
        self.add_action_entries([
            launch_uri_action,
            show_in_files_action,
            quit_action,
            show_preferences_action,
            show_requirements_report_action,
            show_about_action,
        ]);
    }

    fn setup_accels(&self) {
        self.set_accels_for_action("app.show-preferences", &["<Control>comma"]);
        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
        self.set_accels_for_action("win.record-desktop-audio", &["<Control>a"]);
        self.set_accels_for_action("win.record-microphone", &["<Control>m"]);
        self.set_accels_for_action("win.show-pointer", &["<Control>p"]);
        self.set_accels_for_action("win.toggle-record", &["<Control>r"]);
        self.set_accels_for_action("win.toggle-pause", &["<Control>k"]);
        self.set_accels_for_action("win.cancel-record", &["<Control>c"]);
    }
}

/// The tray contents for the window's recording `snapshot` and `settings`.
fn tray_model(snapshot: Option<RecordingSnapshot>, settings: &Settings) -> TrayModel {
    let (state, duration, warnings) = snapshot.map_or(
        (None, gst::ClockTime::ZERO, CaptureWarnings::default()),
        |(state, duration, warnings)| (Some(state), duration, warnings),
    );

    TrayModel {
        state: tray_state(state),
        duration_text: format::digital_clock(duration),
        warnings: warnings.labels(),
        record_desktop_audio: settings.record_desktop_audio(),
        record_microphone: settings.record_microphone(),
        show_pointer: settings.show_pointer(),
        audio_supported: settings
            .profile()
            .is_none_or(|profile| profile.supports_audio()),
        camera_enabled: settings.camera_enabled(),
        camera_available: overlay::availability().is_ok(),
    }
}

/// Applies a capture toggle from the tray to `settings`, which the window's
/// actions share. Returns whether it changed anything.
///
/// The tray disables the toggles during a recording; `has_recording` covers
/// a click that arrives late.
fn apply_tray_command(command: TrayCommand, settings: &Settings, has_recording: bool) -> bool {
    if has_recording {
        return false;
    }

    match command {
        TrayCommand::ToggleDesktopAudio => {
            settings.set_record_desktop_audio(!settings.record_desktop_audio());
        }
        TrayCommand::ToggleMicrophone => {
            settings.set_record_microphone(!settings.record_microphone());
        }
        TrayCommand::TogglePointer => settings.set_show_pointer(!settings.show_pointer()),
        TrayCommand::ToggleRecord
        | TrayCommand::TogglePause
        | TrayCommand::ToggleCamera
        | TrayCommand::ShowWindow
        | TrayCommand::Quit => return false,
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        recording::RecordingState,
        settings::test_utils::{init_resources, settings_lock, setup_schema},
        tray::{
            TrayCommand,
            model::{TrayEntry, TrayRecordingState, menu_entries},
        },
    };

    const CAPTURE_KEYS: [&str; 3] = ["record-desktop-audio", "record-microphone", "show-pointer"];

    fn reset_capture_keys(settings: &Settings) {
        for key in CAPTURE_KEYS {
            settings.reset(key);
        }
    }

    /// Building the window must not end up creating another window: its
    /// `constructed` updates the tray before the app lists it.
    #[gtk::test]
    fn app_and_window_construct() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let app = Application::new();
        let window = Window::new(&app);

        assert!(window.recording_snapshot().is_none());
        assert!(app.imp().tray.get().is_none());
    }

    #[gtk::test]
    fn requirements_report_action() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let app = Application::new();
        app.setup_gactions();

        assert!(app.lookup_action("show-requirements-report").is_some());
    }

    #[test]
    fn apply_tray_command_syncs_window_action() {
        let _lock = settings_lock();
        setup_schema();

        let settings = Settings::default();
        reset_capture_keys(&settings);
        let action = settings.create_record_microphone_action();
        let before = settings.record_microphone();

        assert!(apply_tray_command(
            TrayCommand::ToggleMicrophone,
            &settings,
            false
        ));
        assert_eq!(settings.record_microphone(), !before);
        assert_eq!(action.state(), Some((!before).to_variant()));

        assert!(!apply_tray_command(
            TrayCommand::ToggleMicrophone,
            &settings,
            true
        ));
        assert_eq!(settings.record_microphone(), !before);

        reset_capture_keys(&settings);
    }

    #[gtk::test]
    fn tray_shows_starting_while_portal_pending() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        reset_capture_keys(&settings);
        settings.reset("profile-id");

        let model = tray_model(
            Some((
                RecordingState::Init,
                gst::ClockTime::ZERO,
                CaptureWarnings::default(),
            )),
            &settings,
        );

        assert_eq!(model.state, TrayRecordingState::Starting);
        let check_states = menu_entries(&model)
            .into_iter()
            .filter_map(|entry| match entry {
                TrayEntry::Check {
                    command, enabled, ..
                } if command != TrayCommand::ToggleCamera => Some(enabled),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(check_states, [false, false, false]);
    }

    #[gtk::test]
    fn tray_toggles_camera_also_while_recording() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        // Without `startup` there is no `CameraPip`, so nothing opens the camera.
        let app = Application::new();
        let settings = app.settings();
        settings.reset("camera-enabled");

        app.handle_tray_command(TrayRequest {
            command: TrayCommand::ToggleCamera,
            activation_token: None,
        });
        assert!(settings.camera_enabled());
        assert!(tray_model(None, settings).camera_enabled);
        assert_eq!(
            tray_model(None, settings).camera_available,
            overlay::availability().is_ok()
        );

        app.handle_tray_command(TrayRequest {
            command: TrayCommand::ToggleCamera,
            activation_token: None,
        });
        assert!(!settings.camera_enabled());

        settings.reset("camera-enabled");
    }
}
