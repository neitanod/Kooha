//! The saved areas, the last marked one and the chosen one, kept in
//! `rectangles.json`.

use std::{
    cell::{Cell, OnceCell, RefCell},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gtk::glib::{self, closure_local, prelude::*, subclass::prelude::*};

use super::{
    file::{self, AreasFile, LoadError, Selected},
    model::{ChosenArea, MarkedArea, SavedArea},
};
use crate::i18n::gettext_f;

/// A saved area already has the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameTaken(pub String);

/// What identifies one version of the file on disk. Every save renames a
/// new file into place, so the inode changes even when two saves land in
/// the same coarse modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    inode: u64,
    len: u64,
}

impl FileStamp {
    fn of(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok(),
            inode: metadata.ino(),
            len: metadata.len(),
        })
    }
}

mod imp {
    use std::sync::LazyLock;

    use glib::subclass::Signal;

    use super::*;

    #[derive(Debug, Default)]
    pub struct AreaStore {
        pub(super) path: OnceCell<PathBuf>,
        pub(super) file: RefCell<AreasFile>,
        /// The version of the file last read or written.
        pub(super) stamp: Cell<Option<FileStamp>>,
        /// Never write the file: it is newer, or broken and impossible to
        /// move aside.
        pub(super) read_only: Cell<bool>,
        pub(super) load_notice: RefCell<Option<LoadError>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AreaStore {
        const NAME: &'static str = "KoohaAreaStore";
        type Type = super::AreaStore;
    }

    impl ObjectImpl for AreaStore {
        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("changed").build(),
                    Signal::builder("save-failed")
                        .param_types([String::static_type()])
                        .build(),
                ]
            });

            SIGNALS.as_ref()
        }
    }
}

glib::wrapper! {
    /// The list of saved areas, the last marked area and the chosen one.
    ///
    /// Every change is saved right away and emits `changed`. A failed save
    /// is logged and emits `save-failed` with a message for the user; the
    /// change stays in memory.
    pub struct AreaStore(ObjectSubclass<imp::AreaStore>);
}

impl AreaStore {
    /// The store of `rectangles.json` in the config directory.
    pub fn load_default() -> Self {
        Self::with_path(file::path())
    }

    /// The store of the areas file at `path`.
    pub fn with_path(path: PathBuf) -> Self {
        let obj = glib::Object::new::<Self>();
        let imp = obj.imp();

        match file::load(&path) {
            Ok(file) => {
                imp.file.replace(file);
                imp.stamp.set(FileStamp::of(&path));
            }
            Err(err) => {
                tracing::warn!("Starting with no areas: {}", err);
                // A newer file must not be overwritten, nor a broken one
                // that couldn't be moved aside.
                imp.read_only.set(!matches!(err, LoadError::Broken { .. }));
                imp.load_notice.replace(Some(err));
            }
        }

        imp.path.set(path).unwrap();
        obj
    }

    /// Why the file couldn't be loaded, once: the next call returns `None`.
    pub fn load_notice(&self) -> Option<LoadError> {
        self.imp().load_notice.take()
    }

    /// Whether changes stay in memory only, as the file can't be written.
    pub fn is_read_only(&self) -> bool {
        self.imp().read_only.get()
    }

    pub fn areas(&self) -> Vec<SavedArea> {
        self.imp().file.borrow().areas.clone()
    }

    pub fn last(&self) -> Option<MarkedArea> {
        self.imp().file.borrow().last.clone()
    }

    pub fn selected(&self) -> Option<Selected> {
        self.imp().file.borrow().selected.clone()
    }

    /// The chosen area. `None` also when the chosen name isn't in the list
    /// any more, as when another Kooha removed it.
    pub fn selected_area(&self) -> Option<ChosenArea> {
        let file = self.imp().file.borrow();
        match file.selected.as_ref()? {
            Selected::Named(name) => find(&file, name).map(|index| ChosenArea {
                name: Some(name.clone()),
                area: file.areas[index].area.clone(),
            }),
            Selected::Last => file
                .last
                .clone()
                .map(|area| ChosenArea { name: None, area }),
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        find(&self.imp().file.borrow(), name).is_some()
    }

    /// Chooses an area. A name that isn't saved, or the last area when
    /// there is none, changes nothing.
    pub fn select(&self, selected: Option<Selected>) {
        let _ = self.update(|file| {
            let exists = match &selected {
                Some(Selected::Named(name)) => find(file, name).is_some(),
                Some(Selected::Last) => file.last.is_some(),
                None => true,
            };
            if !exists {
                tracing::warn!("Can't choose missing area {:?}", selected);
                return Ok(false);
            }
            file.selected = selected;
            Ok(true)
        });
    }

    /// Replaces the last area and chooses it.
    pub fn set_last(&self, area: MarkedArea) {
        let _ = self.update(|file| {
            file.last = Some(area);
            file.selected = Some(Selected::Last);
            Ok(true)
        });
    }

    /// Saves `area` as `name` and chooses it. With `replace`, an area of
    /// the same name is replaced in its place in the list.
    pub fn save_named(&self, name: &str, area: MarkedArea, replace: bool) -> Result<(), NameTaken> {
        self.update(|file| {
            let saved = SavedArea {
                name: name.to_string(),
                area,
            };
            match find(file, name) {
                Some(_) if !replace => return Err(NameTaken(name.to_string())),
                Some(index) => file.areas[index] = saved,
                None => file.areas.push(saved),
            }
            file.selected = Some(Selected::Named(name.to_string()));
            Ok(true)
        })
    }

    /// Renames a saved area, keeping it chosen if it was.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), NameTaken> {
        self.update(|file| {
            let Some(index) = find(file, from) else {
                return Ok(false);
            };
            if from == to {
                return Ok(false);
            }
            if find(file, to).is_some() {
                return Err(NameTaken(to.to_string()));
            }
            file.areas[index].name = to.to_string();
            if file.selected == Some(Selected::Named(from.to_string())) {
                file.selected = Some(Selected::Named(to.to_string()));
            }
            Ok(true)
        })
    }

    /// Removes a saved area. If it was chosen, none is chosen any more.
    pub fn remove(&self, name: &str) {
        let _ = self.update(|file| {
            let Some(index) = find(file, name) else {
                return Ok(false);
            };
            file.areas.remove(index);
            if file.selected == Some(Selected::Named(name.to_string())) {
                file.selected = None;
            }
            Ok(true)
        });
    }

    /// Marks a saved area again: new geometry, same name and place.
    pub fn replace_geometry(&self, name: &str, area: MarkedArea) {
        let _ = self.update(|file| {
            let Some(index) = find(file, name) else {
                return Ok(false);
            };
            file.areas[index].area = area;
            Ok(true)
        });
    }

    pub fn connect_changed<F>(&self, f: F) -> glib::SignalHandlerId
    where
        F: Fn(&Self) + 'static,
    {
        self.connect_closure(
            "changed",
            false,
            closure_local!(|obj: &Self| {
                f(obj);
            }),
        )
    }

    pub fn connect_save_failed<F>(&self, f: F) -> glib::SignalHandlerId
    where
        F: Fn(&Self, &str) + 'static,
    {
        self.connect_closure(
            "save-failed",
            false,
            closure_local!(|obj: &Self, message: String| {
                f(obj, &message);
            }),
        )
    }

    fn path(&self) -> &Path {
        self.imp().path.get().unwrap()
    }

    /// Applies `op` to the file as it is on disk, then saves and emits
    /// `changed`. `op` returns whether it changed something, or an error
    /// before changing anything.
    fn update(
        &self,
        op: impl FnOnce(&mut AreasFile) -> Result<bool, NameTaken>,
    ) -> Result<(), NameTaken> {
        let reloaded = self.reload_if_changed();

        let result = op(&mut self.imp().file.borrow_mut());

        let changed = matches!(result, Ok(true));
        if changed {
            self.save();
        }
        if changed || reloaded {
            self.emit_by_name::<()>("changed", &[]);
        }
        result.map(|_| ())
    }

    /// Reads the file again if another Kooha wrote it since this one last
    /// read or wrote it. Returns whether the areas in memory changed.
    fn reload_if_changed(&self) -> bool {
        let imp = self.imp();
        if imp.read_only.get() {
            return false;
        }

        let stamp = FileStamp::of(self.path());
        if stamp == imp.stamp.get() {
            return false;
        }

        match file::load(self.path()) {
            Ok(file) => {
                tracing::debug!("Reloading `{}`, changed on disk", self.path().display());
                imp.file.replace(file);
                imp.stamp.set(stamp);
                true
            }
            Err(err) => {
                tracing::warn!("Keeping the areas in memory: {}", err);
                if !matches!(err, LoadError::Broken { .. }) {
                    imp.read_only.set(true);
                }
                imp.stamp.set(FileStamp::of(self.path()));
                false
            }
        }
    }

    fn save(&self) {
        let imp = self.imp();
        let path = self.path();

        let result = if imp.read_only.get() {
            Err(gettext_f(
                "The areas weren’t saved: {path} can’t be written",
                &[("path", &path.display().to_string())],
            ))
        } else {
            file::save(path, &imp.file.borrow()).map_err(|err| {
                gettext_f(
                    "The areas weren’t saved: {error}",
                    &[("error", &err.to_string())],
                )
            })
        };

        match result {
            Ok(()) => imp.stamp.set(FileStamp::of(path)),
            Err(message) => {
                tracing::error!("Failed to save `{}`: {}", path.display(), message);
                self.emit_by_name::<()>("save-failed", &[&message]);
            }
        }
    }
}

fn find(file: &AreasFile, name: &str) -> Option<usize> {
    file.areas.iter().position(|area| area.name == name)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, fs, rc::Rc};

    use super::*;
    use crate::areas::file::{self, test_utils::TempDir};

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

    fn names(store: &AreaStore) -> Vec<String> {
        store.areas().into_iter().map(|a| a.name).collect()
    }

    fn named(name: &str) -> Option<Selected> {
        Some(Selected::Named(name.into()))
    }

    /// Counts the emissions of `changed`.
    fn changes(store: &AreaStore) -> Rc<Cell<u32>> {
        let count = Rc::new(Cell::new(0));
        let counter = count.clone();
        store.connect_changed(move |_| counter.set(counter.get() + 1));
        count
    }

    /// Collects the messages of `save-failed`.
    fn failures(store: &AreaStore) -> Rc<RefCell<Vec<String>>> {
        let messages = Rc::new(RefCell::new(Vec::new()));
        let sink = messages.clone();
        store.connect_save_failed(move |_, message| sink.borrow_mut().push(message.to_string()));
        messages
    }

    #[test]
    fn save_named_selects_and_persists() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());

        store.save_named("Demo", area(100), false).unwrap();

        assert_eq!(names(&store), ["Demo"]);
        assert_eq!(store.selected(), named("Demo"));
        assert_eq!(
            store.selected_area(),
            Some(ChosenArea {
                name: Some("Demo".into()),
                area: area(100)
            })
        );
        assert_eq!(file::load(&dir.file()).unwrap().areas, store.areas());
    }

    #[test]
    fn repeated_name_without_replace_is_taken() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("Demo", area(100), false).unwrap();
        store.set_last(area(5));
        let count = changes(&store);

        assert_eq!(
            store.save_named("Demo", area(200), false),
            Err(NameTaken("Demo".into()))
        );

        assert_eq!(store.areas()[0].area, area(100));
        assert_eq!(store.selected(), Some(Selected::Last));
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn replace_keeps_the_place_in_the_list() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        for (name, x) in [("A", 1), ("B", 2), ("C", 3)] {
            store.save_named(name, area(x), false).unwrap();
        }
        store.select(named("A"));

        store.save_named("B", area(20), true).unwrap();

        assert_eq!(names(&store), ["A", "B", "C"]);
        assert_eq!(store.areas()[1].area, area(20));
        assert_eq!(store.selected(), named("B"));
    }

    #[test]
    fn rename_keeps_the_choice() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("Other", area(1), false).unwrap();
        store.save_named("Demo", area(100), false).unwrap();

        store.rename("Demo", "Talk").unwrap();

        assert_eq!(names(&store), ["Other", "Talk"]);
        assert_eq!(store.selected(), named("Talk"));
        assert_eq!(file::load(&dir.file()).unwrap().selected, named("Talk"));
    }

    #[test]
    fn rename_to_a_taken_name_changes_nothing() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("Other", area(1), false).unwrap();
        store.save_named("Demo", area(100), false).unwrap();
        let count = changes(&store);

        assert_eq!(
            store.rename("Demo", "Other"),
            Err(NameTaken("Other".into()))
        );

        assert_eq!(names(&store), ["Other", "Demo"]);
        assert_eq!(store.selected(), named("Demo"));
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn removing_the_chosen_one_leaves_none_chosen() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("A", area(1), false).unwrap();
        store.save_named("B", area(2), false).unwrap();

        store.remove("B");

        assert_eq!(names(&store), ["A"]);
        assert_eq!(store.selected(), None);
        assert_eq!(store.selected_area(), None);
        assert_eq!(file::load(&dir.file()).unwrap().selected, None);
    }

    #[test]
    fn removing_another_one_keeps_the_choice() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("A", area(1), false).unwrap();
        store.save_named("B", area(2), false).unwrap();
        store.select(named("A"));

        store.remove("B");

        assert_eq!(store.selected(), named("A"));
        assert!(!store.contains("B"));
        assert!(store.contains("A"));
    }

    #[test]
    fn set_last_replaces_the_last_one_and_chooses_it() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("Demo", area(100), false).unwrap();

        store.set_last(area(1));
        store.set_last(area(2));

        assert_eq!(store.last(), Some(area(2)));
        assert_eq!(store.selected(), Some(Selected::Last));
        assert_eq!(
            store.selected_area(),
            Some(ChosenArea {
                name: None,
                area: area(2)
            })
        );
        assert_eq!(names(&store), ["Demo"]);
    }

    #[test]
    fn replace_geometry_keeps_name_and_choice() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("Demo", area(100), false).unwrap();
        store.save_named("Other", area(1), false).unwrap();

        store.replace_geometry("Demo", area(300));

        assert_eq!(names(&store), ["Demo", "Other"]);
        assert_eq!(store.areas()[0].area, area(300));
        assert_eq!(store.selected(), named("Other"));
    }

    #[test]
    fn choosing_something_missing_changes_nothing() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.save_named("Demo", area(100), false).unwrap();
        let count = changes(&store);

        store.select(named("Nope"));
        store.select(Some(Selected::Last)); // there is no last area

        assert_eq!(store.selected(), named("Demo"));
        assert_eq!(count.get(), 0);

        store.select(None);
        assert_eq!(store.selected(), None);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn changed_is_emitted_once_per_operation() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        let count = changes(&store);

        store.save_named("Demo", area(100), false).unwrap();
        assert_eq!(count.get(), 1);
        store.set_last(area(1));
        assert_eq!(count.get(), 2);
        store.select(named("Demo"));
        assert_eq!(count.get(), 3);
        store.rename("Demo", "Talk").unwrap();
        assert_eq!(count.get(), 4);
        store.replace_geometry("Talk", area(5));
        assert_eq!(count.get(), 5);
        store.remove("Talk");
        assert_eq!(count.get(), 6);
        store.remove("Talk"); // already gone
        assert_eq!(count.get(), 6);
    }

    #[test]
    fn reloading_from_the_same_path_recovers_everything() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.set_last(area(1));
        store.save_named("A", area(10), false).unwrap();
        store.save_named("B", area(20), false).unwrap();
        store.select(named("A"));

        let reloaded = AreaStore::with_path(dir.file());

        assert_eq!(reloaded.areas(), store.areas());
        assert_eq!(reloaded.last(), Some(area(1)));
        assert_eq!(reloaded.selected(), named("A"));
        assert!(reloaded.load_notice().is_none());
        assert!(!reloaded.is_read_only());
    }

    #[test]
    fn broken_file_starts_empty_and_tells_once() {
        let dir = TempDir::new();
        let path = dir.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();

        let store = AreaStore::with_path(path.clone());

        assert!(store.areas().is_empty());
        let Some(LoadError::Broken { moved_to }) = store.load_notice() else {
            panic!("A broken file must be noticed");
        };
        assert_eq!(fs::read_to_string(moved_to).unwrap(), "{ not json");
        assert!(store.load_notice().is_none());
        assert!(!store.is_read_only());

        store.save_named("Demo", area(100), false).unwrap();
        assert_eq!(file::load(&path).unwrap().areas.len(), 1);
    }

    #[test]
    fn newer_file_makes_the_store_read_only() {
        let dir = TempDir::new();
        let path = dir.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let contents = r#"{ "version": 2, "areas": [] }"#;
        fs::write(&path, contents).unwrap();

        let store = AreaStore::with_path(path.clone());
        let failed = failures(&store);
        let count = changes(&store);

        assert!(store.is_read_only());
        assert!(matches!(
            store.load_notice(),
            Some(LoadError::UnsupportedVersion(2))
        ));

        store.set_last(area(1));

        assert_eq!(store.last(), Some(area(1)));
        assert_eq!(count.get(), 1);
        assert_eq!(failed.borrow().len(), 1);
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }

    #[test]
    fn failed_save_keeps_the_change_and_tells() {
        let dir = TempDir::new();
        // The parent "directory" is a regular file: nothing can be written.
        let blocker = dir.0.join("kooha");
        fs::write(&blocker, "").unwrap();
        let store = AreaStore::with_path(blocker.join("rectangles.json"));
        let failed = failures(&store);
        let count = changes(&store);

        store.save_named("Demo", area(100), false).unwrap();

        assert_eq!(names(&store), ["Demo"]);
        assert_eq!(count.get(), 1);
        assert_eq!(failed.borrow().len(), 1);
        assert!(!failed.borrow()[0].is_empty());
    }

    #[test]
    fn writes_of_another_instance_are_kept() {
        let dir = TempDir::new();
        let first = AreaStore::with_path(dir.file());
        let second = AreaStore::with_path(dir.file());
        first.save_named("Kept", area(1), false).unwrap();

        second.save_named("Mine", area(2), false).unwrap();

        assert_eq!(names(&second), ["Kept", "Mine"]);
        let on_disk = file::load(&dir.file()).unwrap();
        assert_eq!(
            on_disk
                .areas
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["Kept", "Mine"]
        );

        // And the other way around, with the file already there.
        first.rename("Kept", "Renamed").unwrap();
        assert_eq!(names(&first), ["Renamed", "Mine"]);
    }

    #[test]
    fn a_taken_name_is_checked_against_the_file() {
        let dir = TempDir::new();
        let first = AreaStore::with_path(dir.file());
        let second = AreaStore::with_path(dir.file());
        first.save_named("Demo", area(1), false).unwrap();

        assert_eq!(
            second.save_named("Demo", area(2), false),
            Err(NameTaken("Demo".into()))
        );
        assert_eq!(names(&second), ["Demo"]);
    }

    #[test]
    fn chosen_area_by_name_or_last() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());
        store.set_last(area(1));
        store.save_named("Demo", area(100), false).unwrap();

        assert_eq!(
            store.selected_area(),
            Some(ChosenArea {
                name: Some("Demo".into()),
                area: area(100)
            })
        );

        store.select(Some(Selected::Last));
        assert_eq!(
            store.selected_area(),
            Some(ChosenArea {
                name: None,
                area: area(1)
            })
        );
    }

    #[test]
    fn chosen_name_removed_elsewhere_is_no_area() {
        let dir = TempDir::new();
        let path = dir.file();
        let file = AreasFile {
            selected: named("Gone"),
            areas: vec![SavedArea {
                name: "Demo".into(),
                area: area(100),
            }],
            ..AreasFile::default()
        };
        file::save(&path, &file).unwrap();

        let store = AreaStore::with_path(path);

        assert_eq!(store.selected(), named("Gone"));
        assert_eq!(store.selected_area(), None);
    }

    #[test]
    fn failed_write_leaves_the_previous_file_intact() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let path = dir.file();
        let store = AreaStore::with_path(path.clone());
        store.save_named("Demo", area(100), false).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let parent = path.parent().unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o555)).unwrap();
        let failed = failures(&store);

        store.save_named("Other", area(1), false).unwrap();

        fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert_eq!(failed.borrow().len(), 1);
        assert_eq!(names(&store), ["Demo", "Other"]);
        assert_eq!(fs::read_dir(parent).unwrap().count(), 1);
    }
}
