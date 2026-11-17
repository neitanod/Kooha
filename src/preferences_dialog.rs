use std::path::Path;

use adw::{prelude::*, subclass::prelude::*};
use gettextrs::gettext;
use gtk::{
    gdk, gio,
    glib::{self, BoxedAnyObject, clone},
};

use crate::{
    camera_pip::{
        self,
        devices::{self, CameraSource},
    },
    experimental::Feature,
    format,
    item_row::ItemRow,
    overlay::{self, OverlayUnavailable},
    profile::Profile,
    settings::{CameraShape, Settings},
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
        pub(super) framerate_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) pause_flash_row: TemplateChild<adw::SwitchRow>,
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
                .bind_camera_enabled(&self.camera_enabled_row.get(), "active")
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

        if let Err(reason) = overlay::availability() {
            imp.overlays_warning_row.set_visible(true);
            imp.overlays_warning_row
                .set_subtitle(&overlay_unavailable_text(reason));

            for row in [
                imp.camera_enabled_row.upcast_ref::<gtk::Widget>(),
                imp.camera_source_row.upcast_ref(),
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

    const CAMERA_KEYS: [&str; 4] = [
        "camera-enabled",
        "camera-device",
        "camera-shape",
        "camera-monitor",
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
            imp.camera_shape_row.upcast_ref(),
            imp.camera_monitor_row.upcast_ref(),
        ] {
            assert_eq!(row.is_sensitive(), availability.is_ok());
        }
        // Connected with the requirements report.
        assert!(!imp.open_report_button.is_visible());

        reset_camera_keys(&settings);
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
