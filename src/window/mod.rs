mod area_row;
mod progress_icon;
mod toggle_button;

use adw::{prelude::*, subclass::prelude::*};
use anyhow::{Error, Result};
use gettextrs::gettext;
use gtk::{
    gdk, gio,
    glib::{self, clone},
};

use std::{
    cell::{Cell, RefCell},
    fmt,
    future::Future,
    pin::Pin,
    rc::Rc,
};

use self::{progress_icon::ProgressIcon, toggle_button::ToggleButton};
use crate::{
    Application,
    area_border::{self, AreaBorder, geometry::BorderGeometry},
    area_marker::{
        self,
        save_dialog::{self, SaveChoice},
    },
    areas::{self, MarkedArea, NameTaken},
    camera_pip::{self, devices::CameraSource},
    cancelled::Cancelled,
    capture_warnings::{CaptureWarnings, WarningsNotifier},
    config::PROFILE,
    format,
    help::ContextWithHelp,
    i18n::gettext_f,
    overlay,
    preferences_dialog::PreferencesDialog,
    recording::{
        NoProfileError, Recording, RecordingState, StaleArea, WrongMonitor, needs_marking,
    },
    settings::CaptureMode,
};

/// What the tray shows of the window's recording: its state, its duration
/// and its capture warnings.
pub type RecordingSnapshot = (RecordingState, gst::ClockTime, CaptureWarnings);

/// What a marking is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkTarget {
    /// A new area, which the user may save with a name.
    New,
    /// New geometry for the saved area of this name.
    Replace(String),
}

/// Marks an area on the screen, starting from the given one; tests replace
/// `area_marker::mark` with it.
pub type Marker =
    Rc<dyn Fn(Option<MarkedArea>) -> Pin<Box<dyn Future<Output = Result<MarkedArea>>>>>;

/// Asks what to do with a marked area, as `save_dialog` does: with a name,
/// again after saving as that name found it taken. Tests replace
/// `save_dialog` with it.
pub type SaveAsker =
    Rc<dyn Fn(MarkedArea, Option<String>) -> Pin<Box<dyn Future<Output = SaveChoice>>>>;

/// What tests put in place of the marker and of the save dialog.
#[derive(Default)]
struct TestHooks {
    marker: Option<Marker>,
    save_asker: Option<SaveAsker>,
}

/// The area border on screen, with what it was built from.
#[derive(Debug)]
struct ShownBorder {
    // Dropping it destroys the layer.
    _layer: AreaBorder,
    geometry: BorderGeometry,
    connector: Option<String>,
}

impl fmt::Debug for TestHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestHooks")
            .field("marker", &self.marker.is_some())
            .field("save_asker", &self.save_asker.is_some())
            .finish()
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/seadve/Kooha/ui/window.ui")]
    pub struct Window {
        #[template_child]
        pub(super) toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub(super) title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) main_page: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub(super) area_row: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) area_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub(super) mark_area_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) camera_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub(super) start_record_button: TemplateChild<gtk::Button>,
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
        /// The border of the chosen or the recorded area, while it is on
        /// screen.
        pub(super) area_border: RefCell<Option<ShownBorder>>,
        /// What each entry of the areas dropdown chooses.
        pub(super) area_items: RefCell<Vec<areas::Selected>>,
        /// The dropdown is being refilled: its selection is not the user's.
        pub(super) updating_area_dropdown: Cell<bool>,
        /// An area is being marked.
        pub(super) marking: Cell<bool>,
        pub(super) hooks: RefCell<TestHooks>,
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

            klass.install_action_async("win.mark-area", None, |obj, _, _| async move {
                obj.mark_area(MarkTarget::New).await;
            });

            klass.install_action_async(
                "win.remark-area",
                Some(glib::VariantTy::STRING),
                |obj, _, name| async move {
                    let Some(name) = name.and_then(|name| name.get::<String>()) else {
                        return;
                    };
                    obj.mark_area(MarkTarget::Replace(name)).await;
                },
            );

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
            obj.setup_areas();

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

    pub fn add_toast(&self, toast: adw::Toast) {
        self.imp().toast_overlay.add_toast(toast);
    }

    /// Marks an area for `target` and saves it in the store: a new one as
    /// the last area or with a name, or the new geometry of a saved one.
    /// Returns `None` if the marking was cancelled, or if marking is
    /// disabled: another marking is open, or there is a recording.
    ///
    /// This is the only caller of `area_marker::mark`. While it runs,
    /// neither marking nor recording can start.
    pub async fn mark_area(&self, target: MarkTarget) -> Option<MarkedArea> {
        let imp = self.imp();
        // The toast's «Mark Again» calls this directly, past the disabled
        // action (codex code r1 #3, fable code r1 #2).
        if !self.enabled_actions().mark_area {
            tracing::debug!("Marking is disabled now");
            return None;
        }
        if imp.marking.replace(true) {
            tracing::debug!("An area is already being marked");
            return None;
        }
        self.update_actions();

        let marked = self.mark_area_inner(target).await;

        imp.marking.set(false);
        self.update_actions();
        marked
    }

    async fn mark_area_inner(&self, target: MarkTarget) -> Option<MarkedArea> {
        let store = Application::get().area_store();

        match target {
            MarkTarget::New => {
                let mut initial = store.selected_area().map(|chosen| chosen.area);
                'mark: loop {
                    let area = self.run_marker(initial.take()).await?;

                    let mut choice = self.ask_save(&area, None).await;
                    loop {
                        match choice {
                            SaveChoice::Last => store.set_last(area.clone()),
                            // Replaces only what the user agreed to replace
                            // (codex code r1 #2).
                            SaveChoice::Named { name, replace } => {
                                if let Err(NameTaken(taken)) =
                                    store.save_named(&name, area.clone(), replace)
                                {
                                    // Another Kooha saved the name after
                                    // the dialog checked it.
                                    choice = self.ask_save(&area, Some(taken)).await;
                                    continue;
                                }
                            }
                            SaveChoice::MarkAgain => {
                                initial = Some(area);
                                continue 'mark;
                            }
                        }
                        return Some(area);
                    }
                }
            }
            MarkTarget::Replace(name) => {
                let Some(saved) = store.areas().into_iter().find(|saved| saved.name == name) else {
                    tracing::warn!("Can't mark missing area {:?} again", name);
                    return None;
                };
                let area = self.run_marker(Some(saved.area)).await?;
                if store.replace_geometry(&name, area.clone()).is_err() {
                    // Removed or renamed while marking: the new geometry
                    // stays, as the last area (fable code r1 #16).
                    store.set_last(area.clone());
                    self.add_toast(adw::Toast::new(&missing_area_notice(&name)));
                }
                Some(area)
            }
        }
    }

    /// What to do with the marked `area`; with `taken`, after saving it as
    /// that name found the name taken.
    async fn ask_save(&self, area: &MarkedArea, taken: Option<String>) -> SaveChoice {
        let asker = self.imp().hooks.borrow().save_asker.clone();
        if let Some(asker) = asker {
            return asker(area.clone(), taken).await;
        }

        // The dialog goes on the window, which may be minimized if the
        // marking came from the tray.
        self.present();
        let store = Application::get().area_store();
        let name_exists = |name: &str| store.contains(name);
        match taken {
            None => save_dialog::ask(self, area, name_exists).await,
            Some(taken) => save_dialog::ask_again(self, area, name_exists, taken).await,
        }
    }

    /// The area marked on the screen, or `None` if cancelled or failed.
    async fn run_marker(&self, initial: Option<MarkedArea>) -> Option<MarkedArea> {
        let marker = self.imp().hooks.borrow().marker.clone();
        let result = match marker {
            Some(marker) => marker(initial).await,
            None => area_marker::mark(initial).await,
        };

        match result {
            Ok(area) => Some(area),
            Err(err) if err.is::<Cancelled>() => {
                tracing::debug!("{:?}", err);
                None
            }
            Err(err) => {
                tracing::error!("Failed to mark an area: {:?}", err);
                self.add_toast(adw::Toast::new(&err.to_string()));
                None
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn set_marker(&self, marker: Marker) {
        self.imp().hooks.borrow_mut().marker = Some(marker);
    }

    #[cfg(test)]
    pub(crate) fn set_save_asker(&self, asker: SaveAsker) {
        self.imp().hooks.borrow_mut().save_asker = Some(asker);
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
        let dialog = self.recording_error_dialog(err);
        dialog.present(Some(self));
    }

    fn recording_error_dialog(&self, err: &Error) -> adw::AlertDialog {
        const OK_RESPONSE_ID: &str = "ok";
        const REPORT_RESPONSE_ID: &str = "report";

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

        dialog.add_response(REPORT_RESPONSE_ID, &gettext("Open Requirements Report"));
        dialog.add_response(OK_RESPONSE_ID, &gettext("Ok, Got It"));
        dialog.connect_response(
            Some(REPORT_RESPONSE_ID),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    if let Err(err) =
                        WidgetExt::activate_action(&obj, "app.show-requirements-report", None)
                    {
                        tracing::error!("Failed to open the requirements report: {:?}", err);
                    }
                }
            ),
        );
        dialog
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

        // In Selection mode with no area chosen, mark one first (P1).
        let has_area = Application::get().area_store().selected_area().is_some();
        if needs_marking(
            Application::get().settings().capture_mode(),
            overlay::availability().is_ok(),
            has_area,
        ) && self.mark_area(MarkTarget::New).await.is_none()
        {
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
            recording.connect_notice(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, message| {
                    obj.add_toast(adw::Toast::new(message));
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

        // Show `Starting` while the portal dialog is open, and mark nothing
        // meanwhile.
        self.update_tray();
        self.update_inhibit();
        self.update_actions();

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
                } else if let Some(stale) = err.downcast_ref::<StaleArea>() {
                    tracing::debug!("{:?}", err);
                    // The toast is on the window, which may be minimized if
                    // the recording started from the tray.
                    self.present();
                    self.add_toast(self.stale_area_toast(stale));
                } else if let Some(wrong) = err.downcast_ref::<WrongMonitor>() {
                    tracing::debug!("{:?}", err);
                    self.present();
                    self.add_toast(adw::Toast::new(&wrong.to_string()));
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
            self.update_actions();
        } else {
            tracing::warn!("Recording finished but no stored recording");
        }
    }

    /// «The area “Demo” is out of date: DP-3 is not connected», with a
    /// button to mark it again.
    fn stale_area_toast(&self, stale: &StaleArea) -> adw::Toast {
        let toast = adw::Toast::builder()
            .title(stale.message())
            .button_label(gettext("Mark Again"))
            .timeout(0)
            .build();
        let target = stale_area_target(stale);
        toast.connect_button_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                let target = target.clone();
                glib::spawn_future_local(clone!(
                    #[weak]
                    obj,
                    async move {
                        obj.mark_area(target).await;
                    }
                ));
            }
        ));
        toast
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

    fn recording_state(&self) -> RecordingState {
        self.imp()
            .recording
            .borrow()
            .as_ref()
            .map_or(RecordingState::Init, |(recording, _)| recording.state())
    }

    /// The recording and marking actions enabled now.
    fn enabled_actions(&self) -> area_row::EnabledActions {
        let imp = self.imp();
        area_row::enabled_actions(
            self.recording_state(),
            imp.recording.borrow().is_some(),
            imp.marking.get(),
        )
    }

    /// Enables the recording and marking actions, and the areas dropdown,
    /// for the recording and whether an area is being marked.
    fn update_actions(&self) {
        let enabled = self.enabled_actions();
        self.action_set_enabled("win.toggle-record", enabled.toggle_record);
        self.action_set_enabled("win.mark-area", enabled.mark_area);
        self.action_set_enabled("win.remark-area", enabled.mark_area);
        self.update_area_dropdown_sensitivity();
    }

    /// The areas dropdown chooses when there is something to choose and
    /// marking is enabled: the chosen area must not change under a
    /// recording that already read it (fable code r1 #1).
    fn update_area_dropdown_sensitivity(&self) {
        let imp = self.imp();
        let has_items = !imp.area_items.borrow().is_empty();
        imp.area_dropdown
            .set_sensitive(has_items && self.enabled_actions().mark_area);
    }

    fn update_view(&self) {
        let imp = self.imp();

        // TODO disregard ms granularity recording state change

        let state = self.recording_state();

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
        self.update_area_border();

        self.update_actions();
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

    /// Shows the border of the chosen area before recording (P8), and of
    /// the recorded area during the countdown, the recording and the pause
    /// (P14); removes it otherwise. The same border on the same monitor
    /// stays on screen, from the chosen area to the recording.
    fn update_area_border(&self) {
        let imp = self.imp();
        let app = Application::get();
        let settings = app.settings();

        let monitors = current_monitors();
        let fresh_area = app
            .area_store()
            .selected_area()
            .filter(|chosen| chosen.area.staleness(&monitors).is_none());
        let source = area_row::border_source(
            self.recording_state(),
            settings.capture_mode() == CaptureMode::Selection,
            settings.show_area_border(),
            fresh_area.is_some(),
            overlay::availability().is_ok(),
        );

        let wanted = match source {
            area_row::BorderSource::None => None,
            area_row::BorderSource::Recording => imp
                .recording
                .borrow()
                .as_ref()
                .and_then(|(recording, _)| recording.area_border()),
            area_row::BorderSource::Idle => {
                fresh_area.and_then(|chosen| area_border::for_area(&chosen.area, &monitors))
            }
        };
        let Some((geometry, monitor)) = wanted else {
            imp.area_border.take();
            return;
        };

        let connector = monitor.connector().map(String::from);
        if imp
            .area_border
            .borrow()
            .as_ref()
            .is_some_and(|shown| shown.geometry == geometry && shown.connector == connector)
        {
            return;
        }

        // The old layer goes before the new one comes.
        imp.area_border.take();
        let layer = AreaBorder::new(geometry, &monitor);
        // Tests never draw on the screen.
        if !cfg!(test) {
            layer.present();
        }
        imp.area_border.replace(Some(ShownBorder {
            _layer: layer,
            geometry,
            connector,
        }));
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
                obj.update_area_row_visibility();
                obj.update_area_border();
            }
        ));

        settings.connect_show_area_border_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.update_area_border();
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
        self.add_action(&settings.create_show_area_border_action());
    }

    /// Fills the areas row and keeps it up to date, and shows the notices
    /// of the saved areas as toasts.
    fn setup_areas(&self) {
        let imp = self.imp();
        let store = Application::get().area_store();

        imp.area_dropdown.connect_selected_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |dropdown| {
                let imp = obj.imp();
                if imp.updating_area_dropdown.get() {
                    return;
                }
                let selected = usize::try_from(dropdown.selected())
                    .ok()
                    .and_then(|index| imp.area_items.borrow().get(index).cloned());
                // After the notification: the store change refills the
                // dropdown, and a new model while the dropdown notifies its
                // own selection hangs GTK.
                if let Some(selected) = selected {
                    glib::idle_add_local_once(move || {
                        Application::get().area_store().select(Some(selected));
                    });
                }
            }
        ));
        store.connect_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.update_area_dropdown();
                obj.update_area_border();
            }
        ));
        if let Some(display) = gdk::Display::default() {
            // The handlers live as long as the display: only a weak
            // reference to the window.
            let obj = self.downgrade();
            areas::monitors::watch(&display, move || {
                if let Some(obj) = obj.upgrade() {
                    obj.update_area_dropdown();
                    obj.update_area_border();
                }
            });
        }

        let controller = gtk::ShortcutController::new();
        controller.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>b"),
            Some(gtk::NamedAction::new("win.show-area-border")),
        ));
        self.add_controller(controller);

        self.update_area_row_visibility();
        self.update_area_dropdown();

        if let Some(notice) = area_row::notice_for_store(&store) {
            self.add_toast(adw::Toast::new(&notice));
        }
        store.connect_save_failed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, message| {
                obj.add_toast(adw::Toast::new(message));
            }
        ));
    }

    /// The areas row shows in Selection mode, when the marker can show its
    /// layers (ADR 0004).
    fn update_area_row_visibility(&self) {
        let selection = Application::get().settings().capture_mode() == CaptureMode::Selection;
        self.imp()
            .area_row
            .set_visible(selection && overlay::availability().is_ok());
    }

    /// Refills the areas dropdown from the store, with the chosen area
    /// selected.
    fn update_area_dropdown(&self) {
        let imp = self.imp();
        let store = Application::get().area_store();

        let items =
            area_row::dropdown_items(&store.areas(), store.last().as_ref(), &current_monitors());
        let selected = store.selected();
        let position = items
            .iter()
            .position(|(_, item, _)| Some(item) == selected.as_ref())
            .and_then(|position| u32::try_from(position).ok())
            .unwrap_or(gtk::INVALID_LIST_POSITION);
        let labels = items
            .iter()
            .map(|(label, _, _)| label.as_str())
            .collect::<Vec<_>>();

        // Keep the model, and what the popover shows, when only the choice
        // changed.
        let same_labels = imp
            .area_dropdown
            .model()
            .and_downcast::<gtk::StringList>()
            .is_some_and(|model| {
                model.n_items() as usize == labels.len()
                    && labels
                        .iter()
                        .zip(0..)
                        .all(|(label, index)| model.string(index).as_deref() == Some(*label))
            });

        imp.updating_area_dropdown.set(true);
        if !same_labels {
            imp.area_dropdown
                .set_model(Some(&gtk::StringList::new(&labels)));
        }
        if imp.area_dropdown.selected() != position {
            imp.area_dropdown.set_selected(position);
        }
        imp.updating_area_dropdown.set(false);

        imp.area_items
            .replace(items.into_iter().map(|(_, item, _)| item).collect());
        self.update_area_dropdown_sensitivity();
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

/// The toast when a saved area marked again was removed or renamed
/// meanwhile.
fn missing_area_notice(name: &str) -> String {
    gettext_f(
        "The area “{name}” is gone; the new one is the last area",
        &[("name", name)],
    )
}

/// A saved area out of date is marked again with its name; the last area
/// is marked anew.
fn stale_area_target(stale: &StaleArea) -> MarkTarget {
    stale
        .name
        .clone()
        .map_or(MarkTarget::New, MarkTarget::Replace)
}

/// The connected monitors, or none without a display.
fn current_monitors() -> Vec<areas::MonitorInfo> {
    gdk::Display::default()
        .map(|display| areas::monitors::current(&display))
        .unwrap_or_default()
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
    use std::{
        future::{Future, poll_fn},
        pin::{Pin, pin},
        rc::Rc,
        task::Poll,
    };

    use futures_channel::oneshot;

    use super::*;
    use crate::{areas::Selected, camera_pip::devices::CameraSource};

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
    fn stale_area_is_marked_again_by_name() {
        let stale = |name: Option<&str>| StaleArea {
            name: name.map(String::from),
            reason: "DP-3 is not connected".into(),
        };
        assert_eq!(
            stale_area_target(&stale(Some("Demo"))),
            MarkTarget::Replace("Demo".into())
        );
        assert_eq!(stale_area_target(&stale(None)), MarkTarget::New);
    }

    #[gtk::test]
    fn stale_area_toast_offers_to_mark_again() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();

        let toast = window.stale_area_toast(&StaleArea {
            name: Some("Demo".into()),
            reason: "DP-3 is not connected".into(),
        });

        assert_eq!(
            toast.title().as_deref(),
            Some("The area “Demo” is out of date: DP-3 is not connected")
        );
        assert_eq!(toast.button_label().as_deref(), Some("Mark Again"));
        assert_eq!(toast.timeout(), 0);
    }

    /// The border is built, never presented.
    #[gtk::test]
    fn chosen_area_border_follows_mode_and_setting() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        if overlay::availability().is_err() {
            eprintln!("Skipped: layers unavailable");
            return;
        }
        let Some(monitor) = current_monitors().into_iter().next() else {
            eprintln!("Skipped: no monitor");
            return;
        };
        let app = Application::get();
        let settings = app.settings();
        let store = app.area_store();
        let (monitor_width, monitor_height) = monitor.physical_size();
        store.set_last(MarkedArea {
            connector: monitor.connector.clone(),
            x: 10,
            y: 10,
            width: 200,
            height: 160,
            monitor_width,
            monitor_height,
            scale: monitor.scale,
        });
        let shown = || window.imp().area_border.borrow().is_some();

        settings.set_capture_mode(CaptureMode::MonitorWindow);
        assert!(!shown());
        settings.set_capture_mode(CaptureMode::Selection);
        assert!(shown());
        settings.set_show_area_border(false);
        assert!(!shown());
        settings.set_show_area_border(true);
        assert!(shown());

        settings.reset("capture-mode");
        settings.reset("show-area-border");
        assert!(!shown());
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

    /// The first primary menu button under `widget`, depth first.
    fn primary_menu_button(widget: &gtk::Widget) -> Option<gtk::MenuButton> {
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(button) = current
                .downcast_ref::<gtk::MenuButton>()
                .filter(|button| button.is_primary())
            {
                return Some(button.clone());
            }
            if let Some(button) = primary_menu_button(&current) {
                return Some(button);
            }
            child = current.next_sibling();
        }
        None
    }

    #[gtk::test]
    fn primary_menu_lists_requirements_report_after_preferences() {
        let _lock = crate::settings::test_utils::settings_lock();
        crate::settings::test_utils::setup_schema();
        crate::settings::test_utils::init_resources();
        gst::init().unwrap();

        let window = Window::new(&Application::new());

        let menu = primary_menu_button(window.upcast_ref())
            .and_then(|button| button.menu_model())
            .expect("window has the primary menu");
        let section = menu
            .item_link(0, gio::MENU_LINK_SECTION)
            .expect("menu has a section");
        let actions = (0..section.n_items())
            .filter_map(|index| {
                section
                    .item_attribute_value(index, gio::MENU_ATTRIBUTE_ACTION, None)
                    .and_then(|value| value.get::<String>())
            })
            .collect::<Vec<_>>();

        let preferences = actions
            .iter()
            .position(|action| action == "app.show-preferences")
            .unwrap();
        assert_eq!(
            actions.get(preferences + 1).map(String::as_str),
            Some("app.show-requirements-report")
        );
        let label = section
            .item_attribute_value(
                i32::try_from(preferences + 1).unwrap(),
                gio::MENU_ATTRIBUTE_LABEL,
                None,
            )
            .and_then(|value| value.get::<String>());
        assert_eq!(label.as_deref(), Some("_Requirements Report"));
    }

    #[gtk::test]
    fn toasts_cover_the_whole_window() {
        let _lock = crate::settings::test_utils::settings_lock();
        crate::settings::test_utils::setup_schema();
        crate::settings::test_utils::init_resources();
        gst::init().unwrap();

        let window = Window::new(&Application::new());

        let toast_overlay = window.imp().toast_overlay.get();
        assert_eq!(window.content(), Some(toast_overlay.upcast()));
        window.add_toast(adw::Toast::new("Hello"));
    }

    /// Polls `future` once.
    async fn is_pending(mut future: Pin<&mut impl Future>) -> bool {
        poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await
    }

    fn test_window() -> Window {
        crate::settings::test_utils::setup_schema();
        crate::settings::test_utils::init_resources();
        gst::init().unwrap();
        let window = Window::new(&Application::new());

        // Tests share the application, and so the store, of the thread.
        let store = Application::get().area_store();
        for saved in store.areas() {
            store.remove(&saved.name);
        }
        window
    }

    fn area(x: i32) -> MarkedArea {
        MarkedArea {
            connector: "DP-3".into(),
            x,
            y: 80,
            width: 1280,
            height: 720,
            monitor_width: 1680,
            monitor_height: 1050,
            scale: 1.05,
        }
    }

    // The settings lock keeps other tests off the shared settings while
    // this one awaits, on the single test thread of GTK.
    #[allow(clippy::await_holding_lock)]
    #[gtk::test]
    async fn marking_disables_marking_and_recording_until_it_ends() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();

        let (sender, receiver) = oneshot::channel::<Result<MarkedArea>>();
        let receiver = RefCell::new(Some(receiver));
        let initials = Rc::new(RefCell::new(Vec::new()));
        window.set_marker(Rc::new(clone!(
            #[strong]
            initials,
            move |initial| {
                initials.borrow_mut().push(initial);
                let receiver = receiver.borrow_mut().take().expect("marked once");
                Box::pin(async move { receiver.await.unwrap() })
            }
        )));
        let record_button = window.imp().start_record_button.get();
        let mark_button = window.imp().mark_area_button.get();
        assert!(record_button.is_sensitive() && mark_button.is_sensitive());
        // A new area starts from the chosen one.
        Application::get().area_store().set_last(area(100));

        let mut marking = pin!(window.mark_area(MarkTarget::New));
        assert!(is_pending(marking.as_mut()).await);
        assert_eq!(*initials.borrow(), [Some(area(100))]);
        assert!(!record_button.is_sensitive());
        assert!(!mark_button.is_sensitive());

        // A second marking while one is open ends at once.
        assert_eq!(window.mark_area(MarkTarget::New).await, None);

        sender
            .send(Err(Cancelled::new("area marking").into()))
            .unwrap();
        assert_eq!(marking.await, None);
        assert!(record_button.is_sensitive() && mark_button.is_sensitive());
        assert_eq!(initials.borrow().len(), 1);
    }

    // The settings lock keeps other tests off the shared settings while
    // this one awaits, on the single test thread of GTK.
    #[allow(clippy::await_holding_lock)]
    #[gtk::test]
    async fn remarking_a_saved_area_keeps_its_name() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        let store = Application::get().area_store();
        store.save_named("Demo", area(100), false).unwrap();
        store.select(None);

        let initials = Rc::new(RefCell::new(Vec::new()));
        window.set_marker(Rc::new(clone!(
            #[strong]
            initials,
            move |initial| {
                initials.borrow_mut().push(initial);
                Box::pin(async { Ok(area(200)) })
            }
        )));

        let marked = window.mark_area(MarkTarget::Replace("Demo".into())).await;

        assert_eq!(marked, Some(area(200)));
        assert_eq!(*initials.borrow(), [Some(area(100))]);
        let saved = store.areas();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Demo");
        assert_eq!(saved[0].area, area(200));
    }

    // The settings lock keeps other tests off the shared settings while
    // this one awaits, on the single test thread of GTK.
    #[allow(clippy::await_holding_lock)]
    #[gtk::test]
    async fn nothing_marks_while_a_recording_opens_its_session() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        let store = Application::get().area_store();
        store.save_named("Demo", area(100), false).unwrap();
        let markings = Rc::new(Cell::new(0));
        window.set_marker(Rc::new(clone!(
            #[strong]
            markings,
            move |_| {
                markings.set(markings.get() + 1);
                Box::pin(async { Ok(area(200)) })
            }
        )));
        let mark_button = window.imp().mark_area_button.get();
        let dropdown = window.imp().area_dropdown.get();
        assert!(mark_button.is_sensitive() && dropdown.is_sensitive());

        // A recording in `Init`, as while the portal dialog is open.
        window
            .imp()
            .recording
            .replace(Some((Recording::new(), Vec::new())));
        window.update_actions();

        assert!(!mark_button.is_sensitive());
        assert!(!dropdown.is_sensitive());
        // The «Mark Again» button of a toast calls it directly.
        assert_eq!(
            window.mark_area(MarkTarget::Replace("Demo".into())).await,
            None
        );
        assert_eq!(window.mark_area(MarkTarget::New).await, None);
        assert_eq!(markings.get(), 0);
        assert_eq!(store.areas()[0].area, area(100));

        window.imp().recording.take();
        window.update_actions();
        assert!(mark_button.is_sensitive() && dropdown.is_sensitive());
        assert_eq!(
            window.mark_area(MarkTarget::Replace("Demo".into())).await,
            Some(area(200))
        );
        assert_eq!(markings.get(), 1);
    }

    /// A save dialog that answers `answers` in order, and records whether
    /// each question was about a taken name.
    fn answer_save_dialog(
        window: &Window,
        answers: Vec<SaveChoice>,
    ) -> Rc<RefCell<Vec<Option<String>>>> {
        let questions = Rc::new(RefCell::new(Vec::new()));
        let answers = RefCell::new(answers.into_iter());
        window.set_save_asker(Rc::new(clone!(
            #[strong]
            questions,
            move |_, taken| {
                questions.borrow_mut().push(taken);
                let answer = answers.borrow_mut().next().expect("asked once more");
                Box::pin(async move { answer })
            }
        )));
        questions
    }

    // The settings lock keeps other tests off the shared settings while
    // this one awaits, on the single test thread of GTK.
    #[allow(clippy::await_holding_lock)]
    #[gtk::test]
    async fn a_name_taken_meanwhile_asks_again_before_replacing() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        let store = Application::get().area_store();
        window.set_marker(Rc::new(|_| Box::pin(async { Ok(area(200)) })));
        let questions = answer_save_dialog(
            &window,
            vec![
                SaveChoice::Named {
                    name: "Demo".into(),
                    replace: false,
                },
                SaveChoice::Named {
                    name: "Demo".into(),
                    replace: true,
                },
            ],
        );
        // Another Kooha saves «Demo» after the dialog checked the name.
        let other = areas::AreaStore::with_path(store.path().to_path_buf());
        other.save_named("Demo", area(100), false).unwrap();

        let marked = window.mark_area(MarkTarget::New).await;

        assert_eq!(marked, Some(area(200)));
        assert_eq!(*questions.borrow(), [None, Some("Demo".to_string())]);
        assert_eq!(store.areas().len(), 1);
        assert_eq!(store.areas()[0].area, area(200));
        assert_eq!(store.selected(), Some(Selected::Named("Demo".into())));
    }

    // The settings lock keeps other tests off the shared settings while
    // this one awaits, on the single test thread of GTK.
    #[allow(clippy::await_holding_lock)]
    #[gtk::test]
    async fn a_name_taken_meanwhile_can_keep_the_other_area() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        let store = Application::get().area_store();
        window.set_marker(Rc::new(|_| Box::pin(async { Ok(area(200)) })));
        answer_save_dialog(
            &window,
            vec![
                SaveChoice::Named {
                    name: "Demo".into(),
                    replace: false,
                },
                // «Change Name», then another name.
                SaveChoice::Named {
                    name: "Talk".into(),
                    replace: false,
                },
            ],
        );
        let other = areas::AreaStore::with_path(store.path().to_path_buf());
        other.save_named("Demo", area(100), false).unwrap();

        window.mark_area(MarkTarget::New).await;

        let saved = store.areas();
        assert_eq!(saved.len(), 2);
        assert_eq!((saved[0].name.as_str(), &saved[0].area), ("Demo", &area(100)));
        assert_eq!((saved[1].name.as_str(), &saved[1].area), ("Talk", &area(200)));
    }

    // The settings lock keeps other tests off the shared settings while
    // this one awaits, on the single test thread of GTK.
    #[allow(clippy::await_holding_lock)]
    #[gtk::test]
    async fn remarking_an_area_removed_meanwhile_keeps_it_as_the_last_one() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        let store = Application::get().area_store();
        store.save_named("Demo", area(100), false).unwrap();
        // The Areas window removes it while the marking is open.
        window.set_marker(Rc::new(|_| {
            Application::get().area_store().remove("Demo");
            Box::pin(async { Ok(area(200)) })
        }));

        let marked = window.mark_area(MarkTarget::Replace("Demo".into())).await;

        assert_eq!(marked, Some(area(200)));
        assert!(store.areas().is_empty());
        assert_eq!(store.last(), Some(area(200)));
        assert_eq!(store.selected(), Some(Selected::Last));
    }

    #[test]
    fn missing_area_notice_names_it() {
        assert_eq!(
            missing_area_notice("Demo"),
            "The area “Demo” is gone; the new one is the last area"
        );
    }

    #[gtk::test]
    fn dropdown_follows_the_store() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        let store = Application::get().area_store();
        let dropdown = window.imp().area_dropdown.get();

        store.save_named("Demo", area(100), false).unwrap();
        store.set_last(area(200));

        let model = dropdown
            .model()
            .and_downcast::<gtk::StringList>()
            .expect("the dropdown lists strings");
        assert_eq!(model.n_items(), 2);
        assert!(model.string(0).unwrap().contains("Demo · 1280×720 on DP-3"));
        assert!(
            model
                .string(1)
                .unwrap()
                .contains("Last area · 1280×720 on DP-3")
        );
        // `set_last` chose the last area.
        assert_eq!(dropdown.selected(), 1);

        dropdown.set_selected(0);
        while glib::MainContext::default().iteration(false) {}
        assert_eq!(store.selected(), Some(Selected::Named("Demo".into())));
    }

    #[gtk::test]
    fn shortcuts_list_the_area_border_toggle() {
        let _lock = crate::settings::test_utils::settings_lock();
        let window = test_window();
        assert!(window.lookup_action("show-area-border").is_some());

        let builder = gtk::Builder::from_resource("/io/github/seadve/Kooha/shortcuts-dialog.ui");
        let lists_border = builder.objects().iter().any(|object| {
            object.find_property("accelerator").is_some()
                && object.property::<Option<String>>("accelerator").as_deref() == Some("<Control>b")
        });
        assert!(lists_border);
    }

    #[gtk::test]
    fn recording_error_dialog_offers_the_report() {
        let _lock = crate::settings::test_utils::settings_lock();
        crate::settings::test_utils::setup_schema();
        crate::settings::test_utils::init_resources();
        gst::init().unwrap();

        let window = Window::new(&Application::new());
        let dialog = window.recording_error_dialog(&anyhow::anyhow!("Failed to record"));

        assert!(dialog.has_response("report"));
        assert_eq!(dialog.response_label("report"), "Open Requirements Report");
        assert!(dialog.has_response("ok"));
        assert_eq!(dialog.default_response().as_deref(), Some("ok"));
    }
}
