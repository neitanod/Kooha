use std::{fs, path::PathBuf, time::Duration};

use adw::prelude::*;
use anyhow::{Context, Result};
use gettextrs::gettext;
use gsettings_macro::gen_settings;
use gtk::{gio, glib};

use crate::{
    area_selector::{Selection, SelectionContext},
    config::APP_ID,
    profile::Profile,
};

#[gen_settings(file = "./data/io.github.seadve.Kooha.gschema.xml.in")]
#[gen_settings_define(key_name = "selection", arg_type = "Selection", ret_type = "Selection")]
#[gen_settings_define(
    key_name = "selection-context",
    arg_type = "SelectionContext",
    ret_type = "SelectionContext"
)]
#[gen_settings_skip(key_name = "saving-location")]
#[gen_settings_skip(key_name = "framerate")]
#[gen_settings_skip(key_name = "record-delay")]
#[gen_settings_skip(key_name = "profile-id")]
pub struct Settings;

impl Default for Settings {
    fn default() -> Self {
        Self::new(APP_ID)
    }
}

impl Settings {
    /// Opens a `FileDialog` to select a folder and updates
    /// the settings with the selected folder.
    pub async fn select_saving_location(
        &self,
        parent: Option<&impl IsA<gtk::Window>>,
    ) -> Result<()> {
        let dialog = gtk::FileDialog::builder()
            .modal(true)
            .title(gettext("Select Recordings Folder"))
            .initial_folder(&gio::File::for_path(self.saving_location()))
            .build();

        let folder = dialog.select_folder_future(parent).await?;
        let path = folder.path().context("Folder does not have a path")?;
        self.0.set("saving-location", path).unwrap();

        Ok(())
    }

    pub fn saving_location(&self) -> PathBuf {
        let stored_saving_location: PathBuf = self.0.get("saving-location");

        if !stored_saving_location.as_os_str().is_empty() {
            return stored_saving_location;
        }

        let saving_location = default_saving_location_parent();
        let kooha_saving_location = saving_location.join("Kooha");

        if let Err(err) = fs::create_dir_all(&kooha_saving_location) {
            tracing::warn!(
                "Failed to create dir at `{}`: {:?}",
                kooha_saving_location.display(),
                err
            );
            return saving_location;
        }

        kooha_saving_location
    }

    /// The stored recordings folder, or the default one, without creating it.
    pub fn saving_location_unchecked(&self) -> PathBuf {
        let stored_saving_location: PathBuf = self.0.get("saving-location");

        if !stored_saving_location.as_os_str().is_empty() {
            return stored_saving_location;
        }

        default_saving_location_parent().join("Kooha")
    }

    pub fn connect_saving_location_changed(
        &self,
        f: impl Fn(&Self) + 'static,
    ) -> glib::SignalHandlerId {
        self.0
            .connect_changed(Some("saving-location"), move |settings, _| {
                f(&Self(settings.clone()));
            })
    }

    pub fn framerate(&self) -> gst::Fraction {
        self.0.get::<(i32, i32)>("framerate").into()
    }

    pub fn set_framerate(&self, framerate: gst::Fraction) {
        let raw = <(i32, i32)>::from(framerate);
        self.0.set("framerate", raw).unwrap();
    }

    pub fn connect_framerate_changed(&self, f: impl Fn(&Self) + 'static) -> glib::SignalHandlerId {
        self.0
            .connect_changed(Some("framerate"), move |settings, _| {
                f(&Self(settings.clone()));
            })
    }

    pub fn record_delay(&self) -> Duration {
        Duration::from_secs(self.0.get::<u32>("record-delay") as u64)
    }

    pub fn create_record_delay_action(&self) -> gio::Action {
        self.0.create_action("record-delay")
    }

    pub fn bind_record_delay<'a>(
        &'a self,
        object: &'a impl IsA<glib::Object>,
        property: &'a str,
    ) -> gio::BindingBuilder<'a> {
        self.0.bind("record-delay", object, property)
    }

    pub fn set_profile(&self, profile: Option<&Profile>) {
        self.0
            .set_string("profile-id", profile.map_or("", |profile| profile.id()))
            .unwrap();
    }

    pub fn profile(&self) -> Option<&'static Profile> {
        let profile_id = self.0.get::<String>("profile-id");

        if profile_id.is_empty() {
            return None;
        }

        Profile::from_id(&profile_id)
            .inspect_err(|err| {
                tracing::warn!("Failed to get profile with id `{}`: {:?}", profile_id, err);
            })
            .ok()
            .filter(|profile| profile.is_available())
    }

    pub fn connect_profile_changed(&self, f: impl Fn(&Self) + 'static) -> glib::SignalHandlerId {
        self.0
            .connect_changed(Some("profile-id"), move |settings, _| {
                f(&Self(settings.clone()));
            })
    }

    pub fn reset_profile(&self) {
        self.0.reset("profile-id");
    }
}

fn default_saving_location_parent() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Videos).unwrap_or_else(glib::home_dir)
}

#[cfg(test)]
pub(crate) mod test_utils {
    use std::{
        env, fs,
        path::PathBuf,
        process::Command,
        sync::{Mutex, MutexGuard, Once, PoisonError},
    };

    use gtk::{gio, prelude::*};

    use super::Settings;
    use crate::config::{APP_ID, GETTEXT_PACKAGE, RESOURCES_FILE};

    /// Compiles the app schema into a temporary directory and points
    /// `GSettings` at it, with the in-memory backend.
    pub(crate) fn setup_schema() {
        static INIT: Once = Once::new();

        INIT.call_once(|| {
            let template = include_str!("../data/io.github.seadve.Kooha.gschema.xml.in");
            let schema = template
                .replace("@app-id@", APP_ID)
                .replace("@gettext-package@", GETTEXT_PACKAGE);

            let schema_dir =
                env::temp_dir().join(format!("kooha-test-schemas-{}", std::process::id()));
            fs::create_dir_all(&schema_dir).unwrap();
            fs::write(schema_dir.join(format!("{APP_ID}.gschema.xml")), schema).unwrap();

            let output = Command::new("glib-compile-schemas")
                .arg(&schema_dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Failed to compile GSchema for tests: {}",
                String::from_utf8_lossy(&output.stderr)
            );

            unsafe {
                env::set_var("GSETTINGS_SCHEMA_DIR", &schema_dir);
                env::set_var("GSETTINGS_BACKEND", "memory");
            }
        });
    }

    /// Registers the app resources, from `KOOHA_TEST_RESOURCES` if set.
    pub(crate) fn init_resources() {
        static INIT: Once = Once::new();

        INIT.call_once(|| {
            // Register the type eagerly: `Profile::all` otherwise registers it
            // lazily from a test thread while the GTK test thread builds the
            // `Window` class, and the two can deadlock inside GLib.
            gio::ZlibDecompressor::static_type();

            let path =
                env::var("KOOHA_TEST_RESOURCES").unwrap_or_else(|_| RESOURCES_FILE.to_string());
            let res = gio::Resource::load(&path)
                .unwrap_or_else(|err| panic!("Failed to load resources at `{path}`: {err}"));
            gio::resources_register(&res);
        });
    }

    /// Serializes the tests that use `Settings`, as they all share the
    /// process-wide in-memory backend.
    pub(crate) fn settings_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());

        LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Points `saving-location` at a temporary directory, so that tests never
    /// create the default recordings folder in the home directory.
    pub(crate) fn temp_saving_location(settings: &Settings) -> PathBuf {
        let dir = env::temp_dir().join(format!("kooha-test-recordings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        settings.0.set("saving-location", &dir).unwrap();
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::{test_utils::*, *};

    #[test]
    fn saving_location_unchecked_does_not_create() {
        let _lock = settings_lock();
        setup_schema();

        let settings = Settings::default();
        settings.reset("saving-location");

        let missing = std::env::temp_dir().join(format!(
            "kooha-test-missing-folder-{}/nested",
            std::process::id()
        ));
        settings.0.set("saving-location", &missing).unwrap();
        assert_eq!(settings.saving_location_unchecked(), missing);
        assert!(!missing.exists());

        settings.reset("saving-location");
        assert!(settings.saving_location_unchecked().ends_with("Kooha"));
    }

    #[test]
    fn default_profile() {
        let _lock = settings_lock();
        setup_schema();
        init_resources();
        gst::init().unwrap();

        let settings = Settings::default();
        settings.reset_profile();

        assert!(settings.profile().is_some());
        assert!(settings.profile().unwrap().supports_audio());
    }
}
