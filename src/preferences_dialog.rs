use std::{io, path::Path};

use adw::{prelude::*, subclass::prelude::*};
use gettextrs::gettext;
use gtk::{
    gdk, gio,
    glib::{self, BoxedAnyObject, clone},
};

use crate::{
    application::Application,
    camera_pip::{
        self,
        devices::{self, CameraSource},
    },
    encoder::{self, H264Encoder},
    experimental::Feature,
    format,
    i18n::gettext_f,
    item_row::ItemRow,
    overlay::{self, OverlayUnavailable},
    profile::Profile,
    settings::{CameraShape, PauseFlashColor, Settings, VideoEncoder},
};

static BUILTIN_FRAMERATES: &[gst::Fraction] = &[
    gst::Fraction::from_integer(10),
    gst::Fraction::from_integer(20),
    gst::Fraction::from_integer(24),
    gst::Fraction::from_integer(25),
    gst::Fraction::new_raw(30_000, 1001), // 29.97
    gst::Fraction::from_integer(30),
    gst::Fraction::from_integer(48),
    gst::Fraction::from_integer(50),
    gst::Fraction::new_raw(60_000, 1001), // 59.94
    gst::Fraction::from_integer(60),
];

/// The items of the encoder row, in the order of the schema.
const ENCODER_CHOICES: [VideoEncoder; 4] = [
    VideoEncoder::Auto,
    VideoEncoder::Intel,
    VideoEncoder::Nvidia,
    VideoEncoder::Cpu,
];

/// The size the encoder row probes Automatic with.
#[cfg_attr(test, allow(dead_code))]
const ENCODER_PROBE_SIZE: (i32, i32) = (1280, 720);

const ROW_SELECTED_ITEM_NOTIFY_HANDLER_ID_KEY: &str = "kooha-row-selected-item-notify-handler-id";
const SETTINGS_PROFILE_CHANGED_HANDLER_ID_KEY: &str = "kooha-settings-profile-changed-handler-id";

/// Used to represent "none" profile in the profiles model
type NoneProfile = BoxedAnyObject;

mod imp {
    use std::cell::{Cell, OnceCell, RefCell};

    use super::*;

    #[derive(Debug, Default, glib::Properties, gtk::CompositeTemplate)]
    #[properties(wrapper_type = super::PreferencesDialog)]
    #[template(resource = "/io/github/seadve/Kooha/ui/preferences_dialog.ui")]
    pub struct PreferencesDialog {
        #[property(get, set, construct_only)]
        pub(super) settings: OnceCell<Settings>,

        #[template_child]
        pub(super) delay_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub(super) file_chooser_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) profile_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) encoder_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) framerate_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) pause_flash_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) pause_flash_color_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) camera_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub(super) overlays_warning_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub(super) open_report_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) camera_enabled_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) camera_source_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) keep_framerate_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) camera_shape_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) camera_monitor_row: TemplateChild<adw::ComboRow>,

        /// The sources the camera can use.
        pub(super) camera_sources: RefCell<Vec<CameraSource>>,
        /// The `camera-device` id of each `camera_source_row` item.
        pub(super) camera_source_ids: RefCell<Vec<String>>,
        /// The connector of each `camera_monitor_row` item.
        pub(super) camera_monitor_connectors: RefCell<Vec<String>>,
        /// Set while the code, not the user, changes the camera rows.
        pub(super) updating_camera_rows: Cell<bool>,
        /// Whether the encoder row probed the encoders.
        pub(super) encoder_probed: Cell<bool>,
    }

    impl PreferencesDialog {
        /// The model of `camera_source_row`, created on first use.
        pub(super) fn camera_source_model(&self) -> gtk::StringList {
            if let Some(model) = self
                .camera_source_row
                .model()
                .and_downcast::<gtk::StringList>()
            {
                return model;
            }
            let model = gtk::StringList::new(&[]);
            self.camera_source_row.set_model(Some(&model));
            model
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "KoohaPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action_async(
                "preferences.select-saving-location",
                None,
                |obj, _, _| async move {
                    let parent = obj.root().map(|r| r.downcast::<gtk::Window>().unwrap());
                    if let Err(err) = obj.settings().select_saving_location(parent.as_ref()).await
                        && !err
                            .downcast_ref::<glib::Error>()
                            .is_some_and(|error| error.matches(gtk::DialogError::Dismissed))
                    {
                        tracing::error!("Failed to select saving location: {:?}", err);

                        let toast = adw::Toast::new(&gettext("Failed to set recordings folder"));
                        obj.add_toast(toast);
                    }
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for PreferencesDialog {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.setup_rows();

            let settings = obj.settings();
            settings
                .bind_record_delay(&self.delay_row.get(), "value")
                .build();
            settings
                .bind_pause_flash(&self.pause_flash_row.get(), "active")
                .build();
            settings
                .bind_pause_flash(&self.pause_flash_color_row.get(), "sensitive")
                .get_only()
                .build();
            obj.setup_pause_flash_color_row();
            settings
                .bind_camera_enabled(&self.camera_enabled_row.get(), "active")
                .build();
            settings
                .bind_camera_keep_framerate(&self.keep_framerate_row.get(), "active")
                .build();
            settings.connect_saving_location_changed(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.update_file_chooser_label();
                }
            ));
            settings.connect_profile_changed(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.update_profile_row_selected();
                    obj.update_encoder_row_visible();
                }
            ));
            settings.connect_framerate_changed(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.update_framerate_row_selected();
                }
            ));

            obj.update_file_chooser_label();
            obj.update_profile_row_selected();
            obj.update_framerate_row_selected();

            obj.setup_camera_rows();
            obj.setup_encoder_row();

            // Load last active value first in `update_*_row` before connecting to
            // the signal to avoid unnecessary updates.
            self.profile_row.connect_selected_item_notify(clone!(
                #[weak]
                obj,
                move |row| {
                    if let Some(item) = row.selected_item() {
                        let profile = profile_from_obj(&item);
                        obj.settings().set_profile(profile);
                    }
                }
            ));
            self.framerate_row.connect_selected_item_notify(clone!(
                #[weak]
                obj,
                move |row| {
                    if let Some(item) = row.selected_item() {
                        let framerate = item
                            .downcast_ref::<BoxedAnyObject>()
                            .unwrap()
                            .borrow::<gst::Fraction>();
                        obj.settings().set_framerate(*framerate);
                    }
                }
            ));
        }
    }

    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends gtk::Widget, adw::Dialog, adw::PreferencesDialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PreferencesDialog {
    pub fn new(settings: &Settings) -> Self {
        glib::Object::builder()
            .property("settings", settings)
            .build()
    }

    /// Shows `sources` in the camera source row.
    pub fn set_camera_sources(&self, sources: &[CameraSource]) {
        self.imp().camera_sources.replace(sources.to_vec());
        self.update_camera_source_row();
    }

    /// Shows `monitors`, as (connector, label), in the camera monitor row,
    /// after "Automatic": the monitor of the Kooha window.
    pub fn set_camera_monitors(&self, monitors: &[(String, String)]) {
        let imp = self.imp();

        let automatic = (String::new(), gettext("Automatic"));
        let items = std::iter::once(&automatic)
            .chain(monitors)
            .collect::<Vec<_>>();

        imp.camera_monitor_connectors.replace(
            items
                .iter()
                .map(|(connector, _)| connector.clone())
                .collect(),
        );
        let labels = items
            .iter()
            .map(|(_, label)| label.as_str())
            .collect::<Vec<_>>();
        self.update_camera_rows(|| {
            imp.camera_monitor_row
                .set_model(Some(&gtk::StringList::new(&labels)));
        });

        self.update_camera_monitor_row_selected();
    }

    /// Lists the sources and selects the one the camera uses. The items
    /// change in place: this also runs while the row notifies its own
    /// selection, where replacing its model crashes.
    fn update_camera_source_row(&self) {
        let imp = self.imp();

        let (items, position) = camera_source_items(
            &imp.camera_sources.borrow(),
            &self.settings().camera_device(),
        );
        let ids = items
            .iter()
            .map(|source| source.id.clone())
            .collect::<Vec<_>>();

        self.update_camera_rows(|| {
            if *imp.camera_source_ids.borrow() != ids {
                let names = items
                    .iter()
                    .map(|source| source.name.as_str())
                    .collect::<Vec<_>>();
                let model = imp.camera_source_model();
                model.splice(0, model.n_items(), &names);
                imp.camera_source_ids.replace(ids);
            }
            imp.camera_source_row.set_selected(
                position.map_or(gtk::INVALID_LIST_POSITION, |position| position as u32),
            );
        });

        self.update_keep_framerate_subtitle();
    }

    /// Tells whether the chosen camera has the control that "Keep Camera
    /// Frame Rate" changes.
    fn update_keep_framerate_subtitle(&self) {
        let imp = self.imp();

        let sources = imp.camera_sources.borrow();
        // Without a V4L2 node, or with a node without the control.
        let has_control = devices::choose(&sources, &self.settings().camera_device())
            .and_then(|source| camera_pip::v4l2::video_node(&source.id))
            .is_some_and(|node| keep_framerate_control(node).is_ok());
        imp.keep_framerate_row
            .set_subtitle(&keep_framerate_subtitle(has_control));
    }

    fn update_camera_monitor_row_selected(&self) {
        let imp = self.imp();

        let connector = self.settings().camera_monitor();
        let position = imp
            .camera_monitor_connectors
            .borrow()
            .iter()
            .position(|item| *item == connector)
            // Not connected: the PiP falls back to the Kooha window monitor.
            .map_or(0, |position| position as u32);
        self.update_camera_rows(|| imp.camera_monitor_row.set_selected(position));
    }

    fn update_camera_shape_row_selected(&self) {
        let position = match self.settings().camera_shape() {
            CameraShape::Circle => 0,
            CameraShape::RoundedRectangle => 1,
        };
        self.update_camera_rows(|| self.imp().camera_shape_row.set_selected(position));
    }

    /// Runs `f` without writing the camera rows back to the settings.
    fn update_camera_rows(&self, f: impl FnOnce()) {
        let imp = self.imp();

        let was_updating = imp.updating_camera_rows.replace(true);
        f();
        imp.updating_camera_rows.set(was_updating);
    }

    fn setup_camera_rows(&self) {
        let imp = self.imp();
        let settings = self.settings();

        imp.camera_group
            .set_title(&glib::markup_escape_text(&gettext("Camera & Overlays")));

        // Over a window that cannot be resized, the dialog is shown as a
        // window of its own, without the app, where an `app.` action name
        // does not resolve and leaves the button insensitive (fable code r1
        // #1). The report opens over the Kooha window, so this dialog closes
        // first instead of staying on top of it.
        imp.open_report_button.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                if obj.root().is_some() {
                    obj.close();
                }
                // Through the action map: `ActionGroup` activation also
                // needs the app registered, which a test cannot do.
                match Application::get().lookup_action("show-requirements-report") {
                    Some(action) => action.activate(None),
                    None => tracing::warn!("No `show-requirements-report` action"),
                }
            }
        ));

        if let Err(reason) = overlay::availability() {
            imp.overlays_warning_row.set_visible(true);
            imp.overlays_warning_row
                .set_subtitle(&overlay_unavailable_text(reason));

            for row in [
                imp.camera_enabled_row.upcast_ref::<gtk::Widget>(),
                imp.camera_source_row.upcast_ref(),
                imp.keep_framerate_row.upcast_ref(),
                imp.camera_shape_row.upcast_ref(),
                imp.camera_monitor_row.upcast_ref(),
            ] {
                row.set_sensitive(false);
            }
        }

        self.set_camera_sources(&camera_pip::app_sources());
        self.set_camera_monitors(&monitor_labels());
        self.update_camera_shape_row_selected();

        settings.connect_camera_device_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.update_camera_source_row()
        ));
        settings.connect_camera_monitor_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.update_camera_monitor_row_selected()
        ));
        settings.connect_camera_shape_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.update_camera_shape_row_selected()
        ));

        // Connected after loading the current values, as with the profile.
        imp.camera_source_row.connect_selected_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |row| {
                if obj.imp().updating_camera_rows.get() {
                    return;
                }
                let id = obj
                    .imp()
                    .camera_source_ids
                    .borrow()
                    .get(row.selected() as usize)
                    .cloned();
                if let Some(id) = id
                    && id != obj.settings().camera_device()
                {
                    obj.settings().set_camera_device(&id);
                }
            }
        ));
        imp.camera_monitor_row.connect_selected_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |row| {
                if obj.imp().updating_camera_rows.get() {
                    return;
                }
                let connector = obj
                    .imp()
                    .camera_monitor_connectors
                    .borrow()
                    .get(row.selected() as usize)
                    .cloned();
                if let Some(connector) = connector
                    && connector != obj.settings().camera_monitor()
                {
                    obj.settings().set_camera_monitor(&connector);
                }
            }
        ));
        imp.camera_shape_row.connect_selected_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |row| {
                if obj.imp().updating_camera_rows.get() {
                    return;
                }
                let shape = match row.selected() {
                    0 => CameraShape::Circle,
                    1 => CameraShape::RoundedRectangle,
                    _ => return,
                };
                if shape != obj.settings().camera_shape() {
                    obj.settings().set_camera_shape(shape);
                }
            }
        ));
    }

    /// The encoder row: its items, its link to the setting, its visibility
    /// and the subtitle with what Automatic uses (ADR 0006).
    fn setup_encoder_row(&self) {
        let row = self.imp().encoder_row.get();
        let settings = self.settings();

        row.set_factory(Some(&row_factory(
            &row,
            "",
            |list_item| {
                let Some(choice) = ENCODER_CHOICES.get(list_item.position() as usize).copied()
                else {
                    return;
                };
                let missing = choice_encoder(choice).and_then(H264Encoder::missing_element);
                let item_row = list_item.child().unwrap().downcast::<ItemRow>().unwrap();
                item_row.set_title(encoder_item_label(choice, missing));
                // A missing encoder can't be chosen, but a stored one is
                // still shown with its reason.
                list_item.set_selectable(missing.is_none());
                list_item.set_activatable(missing.is_none());
            },
            |_| {},
        )));
        let labels = ENCODER_CHOICES.map(|choice| encoder_item_label(choice, None));
        row.set_model(Some(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        )));

        let position = |encoder: VideoEncoder| {
            ENCODER_CHOICES
                .iter()
                .position(|choice| *choice == encoder)
                .map_or(0, |position| position as u32)
        };
        row.set_selected(position(settings.video_encoder()));
        settings.connect_video_encoder_changed(clone!(
            #[weak]
            row,
            move |settings| {
                let selected = position(settings.video_encoder());
                if row.selected() != selected {
                    row.set_selected(selected);
                }
            }
        ));
        row.connect_selected_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |row| {
                let Some(choice) = ENCODER_CHOICES.get(row.selected() as usize).copied() else {
                    return;
                };
                if choice != obj.settings().video_encoder() {
                    obj.settings().set_video_encoder(choice);
                }
            }
        ));

        self.update_encoder_row_visible();
    }

    /// Says in the encoder row which encoder Automatic uses. It probes the
    /// encoders, which can wake a sleeping GPU.
    fn probe_automatic_encoder(&self) {
        let row = self.imp().encoder_row.get();
        glib::spawn_future_local(clone!(
            #[weak]
            row,
            async move {
                let subtitle = match encoder::resolve(VideoEncoder::Auto, probe_encoder).await {
                    Ok(encoder) => gettext_f(
                        // Translators: Do NOT translate the contents between '{' and '}', this is a variable name.
                        "Automatic uses {encoder}",
                        &[("encoder", encoder.name())],
                    ),
                    Err(_) => gettext("No encoder is working"),
                };
                row.set_subtitle(&subtitle);
            }
        ));
    }

    /// Shows the encoder row only for profiles that encode H.264, and probes
    /// the encoders the first time it shows.
    fn update_encoder_row_visible(&self) {
        let imp = self.imp();
        let uses_h264 = self
            .settings()
            .profile()
            .is_some_and(Profile::uses_h264_encoder);
        imp.encoder_row.set_visible(uses_h264);
        if uses_h264 && !imp.encoder_probed.replace(true) {
            self.probe_automatic_encoder();
        }
    }

    pub fn profile_row_grab_focus(&self) -> bool {
        self.imp().profile_row.grab_focus()
    }

    fn update_file_chooser_label(&self) {
        let imp = self.imp();

        let saving_location = self.settings().saving_location();
        imp.file_chooser_label
            .set_label(&display_path(&saving_location));
        imp.file_chooser_label
            .set_tooltip_text(Some(&saving_location.display().to_string()));
    }

    fn update_profile_row_selected(&self) {
        let imp = self.imp();

        let settings = self.settings();
        let active_profile = settings.profile();

        let model = imp.profile_row.model().unwrap();
        let position = model.iter().position(|item| {
            let item = item.unwrap();
            let profile = profile_from_obj(&item);
            profile.map(|p| p.id()) == active_profile.map(|p| p.id())
        });
        if let Some(position) = position {
            imp.profile_row.set_selected(position as u32);
        } else {
            tracing::error!(
                "Active profile `{:?}` was not found on profile model",
                active_profile.as_ref().map(|p| p.id())
            );
        }
    }

    fn update_framerate_row_selected(&self) {
        let imp = self.imp();

        let settings = self.settings();
        let framerate = settings.framerate();

        let model = imp.framerate_row.model().unwrap();
        let position = model
            .iter::<BoxedAnyObject>()
            .position(|item| *item.unwrap().borrow::<gst::Fraction>() == framerate);
        if let Some(position) = position {
            imp.framerate_row.set_selected(position as u32);
        } else {
            tracing::error!(
                "Active framerate `{:?}` was not found on framerate model",
                framerate
            );
        }
    }

    /// The flash color row: White, then Black, as in the schema.
    fn setup_pause_flash_color_row(&self) {
        let row = self.imp().pause_flash_color_row.get();
        let settings = self.settings();

        let position = |color: PauseFlashColor| match color {
            PauseFlashColor::White => 0,
            PauseFlashColor::Black => 1,
        };
        row.set_selected(position(settings.pause_flash_color()));

        settings.connect_pause_flash_color_changed(clone!(
            #[weak]
            row,
            move |settings| {
                let selected = position(settings.pause_flash_color());
                if row.selected() != selected {
                    row.set_selected(selected);
                }
            }
        ));
        row.connect_selected_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |row| {
                let color = match row.selected() {
                    0 => PauseFlashColor::White,
                    1 => PauseFlashColor::Black,
                    _ => return,
                };
                if color != obj.settings().pause_flash_color() {
                    obj.settings().set_pause_flash_color(color);
                }
            }
        ));
    }

    fn setup_rows(&self) {
        let imp = self.imp();

        let settings = self.settings();

        imp.framerate_row.set_factory(Some(&row_factory(
            &imp.framerate_row,
            &gettext("This frame rate may cause performance issues on the selected format."),
            clone!(
                #[strong]
                settings,
                move |list_item| {
                    let item = list_item.item().unwrap();
                    let item_row = list_item.child().unwrap().downcast::<ItemRow>().unwrap();

                    let framerate = item
                        .downcast_ref::<BoxedAnyObject>()
                        .unwrap()
                        .borrow::<gst::Fraction>();
                    item_row.set_title(format::framerate(*framerate));

                    unsafe {
                        list_item.set_data(
                            SETTINGS_PROFILE_CHANGED_HANDLER_ID_KEY,
                            settings.connect_profile_changed(clone!(
                                #[weak]
                                list_item,
                                move |settings| {
                                    update_framerate_row_shows_warning_icon(settings, &list_item);
                                }
                            )),
                        );
                    }

                    update_framerate_row_shows_warning_icon(&settings, list_item);
                }
            ),
            clone!(
                #[strong]
                settings,
                move |list_item| {
                    unsafe {
                        let handler_id = list_item
                            .steal_data(SETTINGS_PROFILE_CHANGED_HANDLER_ID_KEY)
                            .unwrap();
                        settings.disconnect(handler_id);
                    }
                }
            ),
        )));

        let framerate_model = {
            let items = BUILTIN_FRAMERATES
                .iter()
                .map(|fraction| BoxedAnyObject::new(*fraction))
                .collect::<Vec<_>>();

            let model = gio::ListStore::new::<BoxedAnyObject>();
            model.splice(0, 0, &items);

            let active_framerate = settings.framerate();
            if !BUILTIN_FRAMERATES.contains(&active_framerate) {
                model.append(&BoxedAnyObject::new(active_framerate));
            }

            model
        };
        imp.framerate_row.set_model(Some(&framerate_model));

        imp.profile_row.set_factory(Some(&row_factory(
            &imp.profile_row,
            &gettext("This format is experimental and unsupported."),
            |list_item| {
                let item = list_item.item().unwrap();
                let item_row = list_item.child().unwrap().downcast::<ItemRow>().unwrap();

                let profile = profile_from_obj(&item);
                item_row.set_title(
                    profile.map_or_else(|| gettext("None"), |profile| profile.name().to_string()),
                );
                item_row.set_shows_warning_icon(
                    profile.is_some_and(|profile| profile.is_experimental()),
                );
            },
            |_| {},
        )));

        let active_profile = settings.profile();
        let profile_model = {
            let profiles = Profile::all()
                .inspect_err(|err| tracing::error!("Failed to load profiles: {:?}", err))
                .unwrap_or_default();

            let model = gio::ListStore::new::<glib::Object>();
            model.splice(0, 0, profiles);

            if active_profile.is_none() {
                model.insert(0, &NoneProfile::new(()));
            }

            model
        };
        let filter = gtk::CustomFilter::new(move |obj| {
            profile_from_obj(obj).is_none_or(|profile| {
                (Feature::ExperimentalFormats.is_enabled()
                    || !profile.is_experimental()
                    || active_profile.is_some_and(|active_profile| active_profile == profile))
                    && profile.is_available()
            })
        });
        let profile_filter_model = gtk::FilterListModel::new(Some(profile_model), Some(filter));
        imp.profile_row.set_model(Some(&profile_filter_model));
    }
}

fn row_factory(
    row: &adw::ComboRow,
    warning_tooltip_text: &str,
    bind_cb: impl Fn(&gtk::ListItem) + 'static,
    unbind_cb: impl Fn(&gtk::ListItem) + 'static,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    let warning_tooltip_text = warning_tooltip_text.to_string();
    factory.connect_setup(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();

        let item_row = ItemRow::new();
        item_row.set_warning_tooltip_text(warning_tooltip_text.as_str());

        list_item.set_child(Some(&item_row));
    });

    factory.connect_bind(clone!(
        #[weak]
        row,
        move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();

            let item_row = list_item.child().unwrap().downcast::<ItemRow>().unwrap();

            // Only show the selected icon when it is inside the given row's popover. This assumes that
            // the parent of the given row is not a popover, so we can tell which is which.
            if item_row.ancestor(gtk::Popover::static_type()).is_some() {
                debug_assert!(row.ancestor(gtk::Popover::static_type()).is_none());

                item_row.set_is_on_popover(true);

                unsafe {
                    list_item.set_data(
                        ROW_SELECTED_ITEM_NOTIFY_HANDLER_ID_KEY,
                        row.connect_selected_item_notify(clone!(
                            #[weak]
                            list_item,
                            move |row| {
                                update_item_row_is_selected(row, &list_item);
                            }
                        )),
                    );
                }

                update_item_row_is_selected(&row, list_item);
            } else {
                item_row.set_is_on_popover(false);
            }

            bind_cb(list_item);
        }
    ));

    factory.connect_unbind(clone!(
        #[weak]
        row,
        move |_, list_item| {
            let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();

            unsafe {
                if let Some(handler_id) =
                    list_item.steal_data(ROW_SELECTED_ITEM_NOTIFY_HANDLER_ID_KEY)
                {
                    row.disconnect(handler_id);
                }
            }

            unbind_cb(list_item);
        }
    ));

    factory
}

fn update_item_row_is_selected(row: &adw::ComboRow, list_item: &gtk::ListItem) {
    let item_row = list_item.child().unwrap().downcast::<ItemRow>().unwrap();

    item_row.set_is_selected(row.selected_item() == list_item.item());
}

fn update_framerate_row_shows_warning_icon(settings: &Settings, list_item: &gtk::ListItem) {
    let item_row = list_item.child().unwrap().downcast::<ItemRow>().unwrap();
    let item = list_item.item().unwrap();

    let framerate = item
        .downcast_ref::<BoxedAnyObject>()
        .unwrap()
        .borrow::<gst::Fraction>();

    item_row.set_shows_warning_icon(
        settings
            .profile()
            .is_some_and(|profile| *framerate > profile.suggested_max_framerate()),
    );
}

/// The subtitle of the "Keep Camera Frame Rate" row, from whether the chosen
/// camera has `exposure_dynamic_framerate`.
fn keep_framerate_subtitle(has_control: bool) -> String {
    if has_control {
        gettext("In low light the picture gets darker instead of choppy")
    } else {
        gettext("This camera can't change it")
    }
}

/// The `exposure_dynamic_framerate` value of the camera node, read without
/// streaming, which does not turn on the camera light.
#[cfg(not(test))]
fn keep_framerate_control(node: &Path) -> io::Result<i32> {
    camera_pip::v4l2::get_control(node, camera_pip::v4l2::EXPOSURE_DYNAMIC_FRAMERATE)
}

#[cfg(test)]
fn keep_framerate_control(node: &Path) -> io::Result<i32> {
    tests::fake_control(node)
}

/// The H.264 encoder of a choice, `None` for Automatic.
fn choice_encoder(choice: VideoEncoder) -> Option<H264Encoder> {
    match choice {
        VideoEncoder::Auto => None,
        VideoEncoder::Intel => Some(H264Encoder::Intel),
        VideoEncoder::Nvidia => Some(H264Encoder::Nvidia),
        VideoEncoder::Cpu => Some(H264Encoder::Cpu),
    }
}

/// The label of an encoder item, with the element that is not installed.
fn encoder_item_label(choice: VideoEncoder, missing_element: Option<&str>) -> String {
    let name =
        choice_encoder(choice).map_or_else(|| gettext("Automatic"), |e| e.name().to_string());
    match missing_element {
        Some(element) => gettext_f(
            // Translators: Do NOT translate the contents between '{' and '}', this is a variable name.
            "{encoder} ({element} is not installed)",
            &[("encoder", &name), ("element", element)],
        ),
        None => name,
    }
}

#[cfg(not(test))]
async fn probe_encoder(encoder: H264Encoder) -> Result<(), String> {
    encoder.works(ENCODER_PROBE_SIZE).await
}

#[cfg(test)]
thread_local! {
    /// How many times the fake probe ran on this thread.
    static FAKE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Stands for the probe in tests, which never start a hardware encoder:
/// only the CPU works.
#[cfg(test)]
async fn probe_encoder(encoder: H264Encoder) -> Result<(), String> {
    FAKE_PROBES.set(FAKE_PROBES.get() + 1);
    match encoder {
        H264Encoder::Cpu => Ok(()),
        H264Encoder::Intel | H264Encoder::Nvidia => Err(format!("{encoder:?} is fake")),
    }
}

/// Why the camera and the area border are off, for the user.
fn overlay_unavailable_text(reason: OverlayUnavailable) -> String {
    match reason {
        OverlayUnavailable::NotCompiled => gettext("Kooha was built without gtk4-layer-shell."),
        OverlayUnavailable::CompositorUnsupported | OverlayUnavailable::NoDisplay => {
            gettext("Your desktop does not support screen overlays.")
        }
    }
}

/// Each connected monitor as (connector, label).
fn monitor_labels() -> Vec<(String, String)> {
    let Some(display) = gdk::Display::default() else {
        return Vec::new();
    };

    display
        .monitors()
        .iter::<gdk::Monitor>()
        .filter_map(Result::ok)
        .filter_map(|monitor| {
            let connector = monitor.connector()?.to_string();
            let label = match monitor.description().or_else(|| monitor.model()) {
                Some(name) => format!("{name} ({connector})"),
                None => connector.clone(),
            };
            Some((connector, label))
        })
        .collect()
}

/// The items of the camera source row, and the position of the one the
/// camera uses: the configured one, or the first when none is configured. A
/// configured one that is gone gets an item of its own, so that the row
/// neither shows another camera as the chosen one nor keeps that other
/// camera from being picked (codex code r1 #1).
fn camera_source_items(
    sources: &[CameraSource],
    wanted: &str,
) -> (Vec<CameraSource>, Option<usize>) {
    let mut items = sources.to_vec();

    let position = match devices::choose(sources, wanted) {
        Some(chosen) => sources.iter().position(|source| source == chosen),
        None if wanted.is_empty() => None,
        None => {
            items.push(CameraSource {
                id: wanted.to_string(),
                name: gettext("Disconnected Camera"),
            });
            Some(items.len() - 1)
        }
    };

    (items, position)
}

/// Returns `Some` if the object is a `Profile`, otherwise `None`, if the object is a `NoneProfile`.
fn profile_from_obj(obj: &glib::Object) -> Option<&Profile> {
    if let Some(profile) = obj.downcast_ref::<Profile>() {
        Some(profile)
    } else if obj.downcast_ref::<NoneProfile>().is_some() {
        None
    } else {
        tracing::warn!("Unexpected object type `{}`", obj.type_());
        None
    }
}

// Copied from Delineate
// See https://github.com/SeaDve/Delineate/blob/e5f57835133a85c002961e681dc8935249458ef7/src/utils.rs#L71
/// Returns a shortened human-readable representation of the path.
fn display_path(path: &Path) -> String {
    let home_dir = glib::home_dir();

    if path == home_dir {
        return "~/".to_string();
    }

    let path_display = path.display().to_string();

    if path.starts_with(&home_dir) {
        let home_dir_display = home_dir.display().to_string();
        return format!("~{}", &path_display[home_dir_display.len()..]);
    }

    path_display
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        camera_pip::devices::CameraSource, overlay::OverlayUnavailable, settings::CameraShape,
    };

    use crate::settings::test_utils::{init_resources, settings_lock, setup_schema};

    use std::{cell::RefCell, path::PathBuf};

    thread_local! {
        /// The `exposure_dynamic_framerate` value of each fake device node.
        static FAKE_CONTROLS: RefCell<Vec<(PathBuf, i32)>> = const { RefCell::new(Vec::new()) };
    }

    /// Stands for the ioctl in tests, which never open `/dev/video*`: that
    /// could turn on the camera light.
    pub(super) fn fake_control(node: &Path) -> io::Result<i32> {
        FAKE_CONTROLS.with_borrow(|controls| {
            controls
                .iter()
                .find(|(path, _)| path == node)
                .map(|&(_, value)| value)
                .ok_or_else(|| io::ErrorKind::InvalidInput.into())
        })
    }

    #[track_caller]
    fn assert_simplified_framerate(framerate: gst::Fraction) {
        let reduced = framerate.0.reduced();

        assert_eq!(
            (framerate.numer(), framerate.denom()),
            (*reduced.numer(), *reduced.denom())
        );
    }

    #[test]
    fn simplified_framerate() {
        for framerate in BUILTIN_FRAMERATES {
            assert_simplified_framerate(*framerate);
        }
    }

    #[gtk::test]
    fn pause_flash_row_is_bound() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        settings.reset("pause-flash");
        let dialog = PreferencesDialog::new(&settings);
        let row = dialog.imp().pause_flash_row.get();

        assert!(!row.is_active());
        row.set_active(true);
        assert!(settings.pause_flash());
        settings.set_pause_flash(false);
        assert!(!row.is_active());

        settings.reset("pause-flash");
    }

    #[gtk::test]
    fn pause_flash_color_row_follows_setting() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        settings.reset("pause-flash");
        settings.reset("pause-flash-color");
        let dialog = PreferencesDialog::new(&settings);
        let row = dialog.imp().pause_flash_color_row.get();

        // Only usable with the flash on.
        assert!(!row.is_sensitive());
        settings.set_pause_flash(true);
        assert!(row.is_sensitive());

        assert_eq!(row.selected(), 0);
        row.set_selected(1);
        assert_eq!(settings.pause_flash_color(), PauseFlashColor::Black);
        settings.set_pause_flash_color(PauseFlashColor::White);
        assert_eq!(row.selected(), 0);

        settings.reset("pause-flash");
        settings.reset("pause-flash-color");
    }

    const CAMERA_KEYS: [&str; 5] = [
        "camera-enabled",
        "camera-device",
        "camera-shape",
        "camera-monitor",
        "camera-keep-framerate",
    ];

    fn camera_dialog() -> (Settings, PreferencesDialog) {
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        for key in CAMERA_KEYS {
            settings.reset(key);
        }
        let dialog = PreferencesDialog::new(&settings);
        (settings, dialog)
    }

    fn reset_camera_keys(settings: &Settings) {
        for key in CAMERA_KEYS {
            settings.reset(key);
        }
    }

    #[gtk::test]
    fn camera_switch_and_shape_rows_are_bound() {
        let _lock = settings_lock();
        let (settings, dialog) = camera_dialog();
        let imp = dialog.imp();

        assert!(!imp.camera_enabled_row.is_active());
        imp.camera_enabled_row.set_active(true);
        assert!(settings.camera_enabled());

        assert_eq!(imp.camera_shape_row.selected(), 0);
        imp.camera_shape_row.set_selected(1);
        assert_eq!(settings.camera_shape(), CameraShape::RoundedRectangle);
        settings.set_camera_shape(CameraShape::Circle);
        assert_eq!(imp.camera_shape_row.selected(), 0);

        reset_camera_keys(&settings);
    }

    #[gtk::test]
    fn keep_framerate_row_is_bound() {
        let _lock = settings_lock();
        let (settings, dialog) = camera_dialog();
        let imp = dialog.imp();

        // On by default (P15).
        assert!(imp.keep_framerate_row.is_active());
        imp.keep_framerate_row.set_active(false);
        assert!(!settings.camera_keep_framerate());
        settings.set_camera_keep_framerate(true);
        assert!(imp.keep_framerate_row.is_active());

        reset_camera_keys(&settings);
    }

    #[test]
    fn keep_framerate_subtitle_says_when_the_camera_cannot() {
        let can = "In low light the picture gets darker instead of choppy";
        let cannot = "This camera can't change it";
        assert_eq!(keep_framerate_subtitle(true), can);
        assert_eq!(keep_framerate_subtitle(false), cannot);
    }

    /// The subtitle reads the control of the chosen source, through
    /// `FAKE_CONTROLS` instead of the device node.
    #[gtk::test]
    fn keep_framerate_subtitle_follows_the_chosen_camera() {
        let _lock = settings_lock();
        let (settings, dialog) = camera_dialog();
        let imp = dialog.imp();
        FAKE_CONTROLS.with_borrow_mut(|controls| {
            *controls = vec![(PathBuf::from("/dev/video2"), 1)];
        });

        dialog.set_camera_sources(&[
            CameraSource {
                id: "/dev/video2".to_string(),
                name: "Webcam".to_string(),
            },
            CameraSource {
                id: "/dev/video4".to_string(),
                name: "Capture Card".to_string(),
            },
            CameraSource {
                id: "OBS".to_string(),
                name: "OBS Virtual Camera".to_string(),
            },
        ]);
        let subtitle = || imp.keep_framerate_row.subtitle().unwrap_or_default();

        // An empty `camera-device` uses the first source.
        assert_eq!(subtitle(), keep_framerate_subtitle(true));
        settings.set_camera_device("/dev/video4");
        assert_eq!(subtitle(), keep_framerate_subtitle(false));
        settings.set_camera_device("OBS");
        assert_eq!(subtitle(), keep_framerate_subtitle(false));
        settings.set_camera_device("/dev/video2");
        assert_eq!(subtitle(), keep_framerate_subtitle(true));

        FAKE_CONTROLS.with_borrow_mut(Vec::clear);
        reset_camera_keys(&settings);
    }

    #[gtk::test]
    fn camera_source_row_follows_device() {
        let _lock = settings_lock();
        let (settings, dialog) = camera_dialog();
        let imp = dialog.imp();

        dialog.set_camera_sources(&[
            CameraSource {
                id: "/dev/video0".to_string(),
                name: "Webcam".to_string(),
            },
            CameraSource {
                id: "OBS".to_string(),
                name: "OBS Virtual Camera".to_string(),
            },
        ]);

        // An empty `camera-device` uses the first source.
        assert_eq!(imp.camera_source_row.selected(), 0);
        assert_eq!(settings.camera_device(), "");

        imp.camera_source_row.set_selected(1);
        assert_eq!(settings.camera_device(), "OBS");
        settings.set_camera_device("/dev/video0");
        assert_eq!(imp.camera_source_row.selected(), 0);
        // A saved camera that is gone is not shown as another one: it gets
        // an item of its own, and picking a source replaces it.
        settings.set_camera_device("/dev/video9");
        let model = imp
            .camera_source_row
            .model()
            .and_downcast::<gtk::StringList>()
            .unwrap();
        assert_eq!(model.n_items(), 3);
        assert_eq!(imp.camera_source_row.selected(), 2);
        assert_eq!(model.string(2).as_deref(), Some("Disconnected Camera"));
        imp.camera_source_row.set_selected(0);
        assert_eq!(settings.camera_device(), "/dev/video0");
        assert_eq!(imp.camera_source_row.model().unwrap().n_items(), 2);

        reset_camera_keys(&settings);
    }

    #[gtk::test]
    fn camera_monitor_row_follows_connector() {
        let _lock = settings_lock();
        let (settings, dialog) = camera_dialog();
        let imp = dialog.imp();

        dialog.set_camera_monitors(&[
            ("eDP-1".to_string(), "Built-in display (eDP-1)".to_string()),
            ("DP-1".to_string(), "Dell (DP-1)".to_string()),
        ]);

        // An empty `camera-monitor` follows the Kooha window: "Automatic".
        assert_eq!(imp.camera_monitor_row.selected(), 0);

        imp.camera_monitor_row.set_selected(2);
        assert_eq!(settings.camera_monitor(), "DP-1");
        settings.set_camera_monitor("eDP-1");
        assert_eq!(imp.camera_monitor_row.selected(), 1);
        imp.camera_monitor_row.set_selected(0);
        assert_eq!(settings.camera_monitor(), "");
        // A saved monitor that is not connected also shows "Automatic".
        settings.set_camera_monitor("HDMI-1");
        assert_eq!(imp.camera_monitor_row.selected(), 0);

        reset_camera_keys(&settings);
    }

    #[gtk::test]
    fn overlays_warning_follows_availability() {
        let _lock = settings_lock();
        let (settings, dialog) = camera_dialog();
        let imp = dialog.imp();

        let availability = crate::overlay::availability();
        assert_eq!(imp.overlays_warning_row.is_visible(), availability.is_err());
        for row in [
            imp.camera_enabled_row.upcast_ref::<gtk::Widget>(),
            imp.camera_source_row.upcast_ref(),
            imp.keep_framerate_row.upcast_ref(),
            imp.camera_shape_row.upcast_ref(),
            imp.camera_monitor_row.upcast_ref(),
        ] {
            assert_eq!(row.is_sensitive(), availability.is_ok());
        }
        // Reads the button's own `visible`: its row is hidden when overlays
        // are available.
        assert!(imp.open_report_button.get_visible());

        reset_camera_keys(&settings);
    }

    /// The dialog may be shown as a window of its own, without the app, so
    /// the button reaches the app action through `Application` (fable code
    /// r1 #1).
    #[gtk::test]
    fn open_report_button_activates_app_action() {
        use std::{cell::Cell, rc::Rc};

        let _lock = settings_lock();
        let (settings, dialog) = camera_dialog();

        let app = crate::application::Application::new();
        app.set_default();
        let activations = Rc::new(Cell::new(0));
        let action = gio::SimpleAction::new("show-requirements-report", None);
        action.connect_activate(clone!(
            #[strong]
            activations,
            move |_, _| activations.set(activations.get() + 1)
        ));
        app.add_action(&action);

        let button = dialog.imp().open_report_button.get();
        assert_eq!(button.action_name(), None);
        button.emit_clicked();

        assert_eq!(activations.get(), 1);

        reset_camera_keys(&settings);
    }

    /// Resets `profile-id` and leaves no value stored. The first reset moves
    /// the profile row, which stores the default as the user's choice; the
    /// second removes it. A stored value would make a later reset in another
    /// test notify every window still alive.
    fn reset_profile(settings: &Settings) {
        settings.reset("profile-id");
        settings.reset("profile-id");
        assert!(settings.user_value("profile-id").is_none());
    }

    fn encoder_dialog() -> (Settings, PreferencesDialog) {
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        reset_profile(&settings);
        settings.reset("video-encoder");
        let dialog = PreferencesDialog::new(&settings);
        (settings, dialog)
    }

    #[gtk::test]
    fn encoder_row_is_visible_only_for_h264_profiles() {
        let _lock = settings_lock();
        let (settings, dialog) = encoder_dialog();
        let row = dialog.imp().encoder_row.get();

        for (id, visible) in [
            ("mp4", true),
            ("webm-vp8", false),
            ("matroska-h264", true),
            ("gif", false),
        ] {
            settings.set_profile(Some(Profile::from_id(id).unwrap()));
            assert_eq!(row.is_visible(), visible, "{id}");
        }

        reset_profile(&settings);
    }

    #[gtk::test]
    fn encoder_row_follows_setting() {
        let _lock = settings_lock();
        let (settings, dialog) = encoder_dialog();
        let row = dialog.imp().encoder_row.get();

        assert_eq!(row.selected(), 0);
        row.set_selected(3);
        assert_eq!(settings.video_encoder(), VideoEncoder::Cpu);
        settings.set_video_encoder(VideoEncoder::Nvidia);
        assert_eq!(row.selected(), 2);

        settings.reset("video-encoder");
    }

    /// Runs the main context until nothing is pending.
    fn drain_main_context() {
        let context = glib::MainContext::default();
        while context.iteration(false) {}
    }

    /// Waits at most five seconds for `row` to have a subtitle.
    fn wait_for_subtitle(row: &adw::ComboRow) {
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while row.subtitle().is_none_or(|subtitle| subtitle.is_empty())
            && std::time::Instant::now() < deadline
        {
            context.iteration(false);
        }
    }

    #[gtk::test]
    fn encoder_row_says_what_automatic_uses() {
        let _lock = settings_lock();
        let (settings, dialog) = encoder_dialog();
        settings.set_profile(Some(Profile::from_id("mp4").unwrap()));
        let row = dialog.imp().encoder_row.get();

        // The fake probe only passes the CPU.
        wait_for_subtitle(&row);
        assert_eq!(row.subtitle().as_deref(), Some("Automatic uses CPU"));

        reset_profile(&settings);
        settings.reset("video-encoder");
    }

    #[gtk::test]
    fn encoder_row_probes_only_once_visible() {
        let _lock = settings_lock();
        setup_schema();
        drain_main_context();
        let probes_before = FAKE_PROBES.get();

        let (settings, dialog) = encoder_dialog();
        settings.set_profile(Some(Profile::from_id("webm-vp8").unwrap()));
        let row = dialog.imp().encoder_row.get();
        drain_main_context();
        assert!(!row.is_visible());
        assert_eq!(FAKE_PROBES.get(), probes_before);
        assert!(row.subtitle().is_none_or(|subtitle| subtitle.is_empty()));

        settings.set_profile(Some(Profile::from_id("mp4").unwrap()));
        wait_for_subtitle(&row);
        assert_eq!(row.subtitle().as_deref(), Some("Automatic uses CPU"));
        let probes = FAKE_PROBES.get() - probes_before;
        assert_eq!(probes, encoder::AUTO_ORDER.len());

        // Hiding and showing it again does not probe again.
        settings.set_profile(Some(Profile::from_id("gif").unwrap()));
        settings.set_profile(Some(Profile::from_id("matroska-h264").unwrap()));
        drain_main_context();
        assert_eq!(FAKE_PROBES.get() - probes_before, probes);

        reset_profile(&settings);
        settings.reset("video-encoder");
    }

    #[test]
    fn encoder_items_name_the_missing_element() {
        assert_eq!(encoder_item_label(VideoEncoder::Auto, None), "Automatic");
        assert_eq!(encoder_item_label(VideoEncoder::Nvidia, None), "NVIDIA");
        assert_eq!(
            encoder_item_label(VideoEncoder::Intel, Some("vah264lpenc")),
            "Intel (vah264lpenc is not installed)"
        );
    }

    #[test]
    fn overlay_reasons() {
        assert_eq!(
            overlay_unavailable_text(OverlayUnavailable::NotCompiled),
            "Kooha was built without gtk4-layer-shell."
        );
        assert_eq!(
            overlay_unavailable_text(OverlayUnavailable::CompositorUnsupported),
            "Your desktop does not support screen overlays."
        );
    }
}
