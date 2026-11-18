//! The saved areas file, `rectangles.json` in the config directory.

use std::{
    ffi::OsString,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use gtk::glib;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::model::{MarkedArea, SavedArea};

/// The format version this code writes.
const VERSION: u32 = 1;

/// The contents of `rectangles.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AreasFile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<MarkedArea>,
    #[serde(default)]
    pub areas: Vec<SavedArea>,
    /// A name, `null` for the last area, or absent when none is chosen.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_selected",
        deserialize_with = "deserialize_selected"
    )]
    pub selected: Option<Selected>,
}

impl Default for AreasFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            last: None,
            areas: Vec::new(),
            selected: None,
        }
    }
}

/// The chosen area: a saved one by name, or the last marked one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Selected {
    Named(String),
    Last,
}

// Only called for `Some`: `None` leaves the key out.
fn serialize_selected<S: Serializer>(
    selected: &Option<Selected>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match selected {
        Some(Selected::Named(name)) => serializer.serialize_some(name),
        Some(Selected::Last) | None => serializer.serialize_none(),
    }
}

// Only called when the key is present: an absent key is `None` by default.
fn deserialize_selected<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Selected>, D::Error> {
    let name = Option::<String>::deserialize(deserializer)?;
    Ok(Some(name.map_or(Selected::Last, Selected::Named)))
}

/// Why `rectangles.json` couldn't be loaded.
#[derive(Debug)]
pub enum LoadError {
    /// The file was unreadable or invalid, and was renamed to `moved_to`.
    Broken { moved_to: PathBuf },
    /// The file was unreadable or invalid, and couldn't be renamed either.
    Io(io::Error),
    /// The file was written by a newer Kooha, and was left untouched.
    UnsupportedVersion(u32),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Broken { moved_to } => {
                write!(f, "Broken areas file, moved to `{}`", moved_to.display())
            }
            Self::Io(err) => write!(f, "Broken areas file that couldn't be moved: {err}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "Areas file of the newer version {version}")
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Where the areas file lives: `rectangles.json` in Kooha's config
/// directory, which follows `XDG_CONFIG_HOME`.
#[cfg(not(test))]
pub fn path() -> PathBuf {
    glib::user_config_dir()
        .join("kooha")
        .join("rectangles.json")
}

/// In tests, a per-process temporary directory: no test may touch the
/// user's areas. Each call gets its own file, so that a test that saves
/// an area can't change what the next one loads.
#[cfg(test)]
pub fn path() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join(format!("kooha-test-{}", std::process::id()))
        .join(format!("rectangles-{n}.json"))
}

/// Reads the areas file at `path`. A missing file is an empty one. An
/// unreadable or invalid file is renamed to
/// `rectangles.json.broken-<YYYYMMDD-HHMMSS>` so that the next save
/// doesn't overwrite it. A file of a newer version is left as it is.
pub fn load(path: &Path) -> Result<AreasFile, LoadError> {
    let parsed = match fs::read(path) {
        Ok(bytes) => parse(&bytes)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(AreasFile::default()),
        Err(err) => Err(err.to_string()),
    };

    match parsed {
        Ok(file) => Ok(file),
        Err(reason) => {
            tracing::warn!("Failed to load `{}`: {}", path.display(), reason);
            let moved_to = with_suffix(path, &format!(".broken-{}", timestamp()));
            fs::rename(path, &moved_to).map_err(LoadError::Io)?;
            Err(LoadError::Broken { moved_to })
        }
    }
}

/// Parses the file, or tells why it is broken. Checks the version first,
/// so that a newer format that this one can't read isn't taken as broken.
fn parse(bytes: &[u8]) -> Result<Result<AreasFile, String>, LoadError> {
    #[derive(Deserialize)]
    struct Versioned {
        version: u32,
    }

    match serde_json::from_slice::<Versioned>(bytes) {
        Ok(Versioned { version }) if version > VERSION => {
            Err(LoadError::UnsupportedVersion(version))
        }
        Ok(_) => Ok(serde_json::from_slice::<AreasFile>(bytes).map_err(|err| err.to_string())),
        Err(err) => Ok(Err(err.to_string())),
    }
}

/// Writes `file` to `path` atomically: to `<path>.tmp-<pid>` first, synced,
/// then renamed over `path`. Creates the directory if needed. On a failure
/// `path` keeps its previous contents and the temporary file is removed.
pub fn save(path: &Path, file: &AreasFile) -> io::Result<()> {
    save_with(path, file, rename)
}

/// `save()` with the rename that puts the temporary file in place, so that a
/// test can make it fail after the temporary file is written.
fn save_with(
    path: &Path,
    file: &AreasFile,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(file).map_err(io::Error::other)?;
    let tmp = temporary_path(path);
    let result = write_synced(&tmp, json.as_bytes()).and_then(|()| rename(&tmp, path));
    if result.is_err() {
        // Whatever failed, `path` still holds the previous file; the
        // temporary one would only be left behind.
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_synced(path: &Path, json: &[u8]) -> io::Result<()> {
    let mut out = fs::File::create(path)?;
    out.write_all(json)?;
    out.write_all(b"\n")?;
    out.sync_all()
}

#[cfg(not(test))]
fn rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

/// `fs::rename`, unless the test made it fail with `test_utils::FailRename`.
#[cfg(test)]
fn rename(from: &Path, to: &Path) -> io::Result<()> {
    if test_utils::FAIL_RENAME.get() {
        Err(io::Error::other("rename failed on purpose"))
    } else {
        fs::rename(from, to)
    }
}

/// Per process, so that two Kooha instances never write the same one.
fn temporary_path(path: &Path) -> PathBuf {
    with_suffix(path, &format!(".tmp-{}", std::process::id()))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

fn timestamp() -> String {
    glib::DateTime::now_local()
        .and_then(|now| now.format("%Y%m%d-%H%M%S"))
        .map_or_else(|_| "unknown".to_string(), String::from)
}

#[cfg(test)]
pub(crate) mod test_utils {
    use std::{cell::Cell, fs, path::PathBuf};

    use gtk::glib;

    thread_local! {
        pub(super) static FAIL_RENAME: Cell<bool> = const { Cell::new(false) };
    }

    /// While alive, every `save()` on this thread fails at the rename, after
    /// writing the temporary file.
    pub(crate) struct FailRename;

    impl FailRename {
        pub(crate) fn new() -> Self {
            FAIL_RENAME.set(true);
            Self
        }
    }

    impl Drop for FailRename {
        fn drop(&mut self) {
            FAIL_RENAME.set(false);
        }
    }

    /// A temporary directory, removed with its contents on drop.
    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new() -> Self {
            let template = glib::tmp_dir().join("kooha-test-areas-XXXXXX");
            Self(glib::mkdtemp(template).expect("Failed to create a temporary directory"))
        }

        pub(crate) fn file(&self) -> PathBuf {
            self.0.join("kooha").join("rectangles.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{test_utils::TempDir, *};

    fn area(connector: &str, x: i32) -> MarkedArea {
        MarkedArea {
            connector: connector.into(),
            x,
            y: 80,
            width: 1280,
            height: 720,
            monitor_width: 1680,
            monitor_height: 1050,
            scale: 1.05,
        }
    }

    fn sample() -> AreasFile {
        AreasFile {
            version: 1,
            last: Some(area("DP-3", 10)),
            areas: vec![
                SavedArea {
                    name: "Demo".into(),
                    area: area("DP-3", 100),
                },
                SavedArea {
                    name: "Notebook".into(),
                    area: area("eDP-1", 0),
                },
            ],
            selected: Some(Selected::Named("Demo".into())),
        }
    }

    fn write(dir: &TempDir, contents: &str) -> PathBuf {
        let path = dir.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn round_trip() {
        let dir = TempDir::new();
        let path = dir.file();

        save(&path, &sample()).unwrap();

        assert_eq!(load(&path).unwrap(), sample());
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains(r#""selected": "Demo""#), "{json}");
        assert!(json.contains(r#""name": "Demo""#), "{json}");
        assert!(json.contains(r#""monitor_width": 1680"#), "{json}");
    }

    #[test]
    fn last_selected_is_written_as_null() {
        let dir = TempDir::new();
        let path = dir.file();
        let file = AreasFile {
            selected: Some(Selected::Last),
            ..sample()
        };

        save(&path, &file).unwrap();

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains(r#""selected": null"#), "{json}");
        assert_eq!(load(&path).unwrap(), file);
    }

    #[test]
    fn nothing_selected_leaves_the_key_out() {
        let dir = TempDir::new();
        let path = dir.file();
        let file = AreasFile {
            last: None,
            selected: None,
            ..sample()
        };

        save(&path, &file).unwrap();

        let json = fs::read_to_string(&path).unwrap();
        assert!(!json.contains("selected"), "{json}");
        assert!(!json.contains("last"), "{json}");
        assert_eq!(load(&path).unwrap(), file);
    }

    #[test]
    fn selected_null_means_last() {
        let dir = TempDir::new();
        let path = write(&dir, r#"{ "version": 1, "areas": [], "selected": null }"#);

        assert_eq!(load(&path).unwrap().selected, Some(Selected::Last));
    }

    #[test]
    fn selected_absent_means_none() {
        let dir = TempDir::new();
        let path = write(&dir, r#"{ "version": 1, "areas": [] }"#);

        let file = load(&path).unwrap();
        assert_eq!(file.selected, None);
        assert_eq!(file.last, None);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = TempDir::new();

        let file = load(&dir.file()).unwrap();

        assert_eq!(file, AreasFile::default());
        assert_eq!(file.version, 1);
        assert!(file.areas.is_empty());
    }

    #[test]
    fn broken_file_is_moved_aside() {
        let dir = TempDir::new();
        let path = write(&dir, "{ not json");

        let Err(LoadError::Broken { moved_to }) = load(&path) else {
            panic!("A broken file must not load");
        };

        assert!(!path.exists());
        assert!(moved_to.exists());
        assert_eq!(moved_to.parent(), path.parent());
        assert_eq!(fs::read_to_string(&moved_to).unwrap(), "{ not json");
        let name = moved_to.file_name().unwrap().to_str().unwrap();
        let stamp = name
            .strip_prefix("rectangles.json.broken-")
            .unwrap_or_else(|| panic!("Unexpected name `{name}`"));
        let (date, time) = stamp.split_once('-').unwrap();
        assert_eq!(date.len(), 8, "{name}");
        assert_eq!(time.len(), 6, "{name}");
        assert!(date.chars().chain(time.chars()).all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn an_invalid_area_loads_and_stays_in_place() {
        let dir = TempDir::new();
        let path = write(
            &dir,
            r#"{
                "version": 1,
                "areas": [{
                    "name": "Edited", "connector": "DP-3",
                    "x": -2, "y": 80, "width": 1280, "height": 720,
                    "monitor_width": 1680, "monitor_height": 1050, "scale": 1.05
                }]
            }"#,
        );

        let file = load(&path).unwrap();

        // The area itself is out of date; the file isn't broken.
        assert_eq!(file.areas.len(), 1);
        assert_eq!(file.areas[0].area.x, -2);
        assert!(path.exists());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn save_is_atomic() {
        let dir = TempDir::new();
        let path = write(&dir, "old");
        // A hard link keeps the old inode: an in-place write would change it,
        // a write to a temporary file and a rename leaves it alone.
        let link = dir.0.join("old-link");
        fs::hard_link(&path, &link).unwrap();

        save(&path, &sample()).unwrap();

        assert_eq!(fs::read_to_string(&link).unwrap(), "old");
        assert_eq!(load(&path).unwrap(), sample());
        let names = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, ["rectangles.json"]);
    }

    #[test]
    fn failed_rename_keeps_the_previous_file() {
        let dir = TempDir::new();
        let path = write(&dir, "old");
        let mut written = None;

        let err = save_with(&path, &sample(), |tmp, to| {
            assert_eq!(to, path);
            written = Some(fs::read_to_string(tmp).unwrap());
            Err(io::Error::other("rename failed on purpose"))
        })
        .unwrap_err();

        assert_eq!(err.to_string(), "rename failed on purpose");
        let written = written.expect("The rename was never tried");
        assert!(written.contains(r#""name": "Demo""#), "{written}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        let names = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, ["rectangles.json"]);
    }

    #[test]
    fn temporary_file_is_per_process() {
        let path = PathBuf::from("/x/rectangles.json");
        assert_eq!(
            temporary_path(&path),
            PathBuf::from(format!("/x/rectangles.json.tmp-{}", std::process::id()))
        );
    }

    #[test]
    fn newer_version_is_left_alone() {
        let dir = TempDir::new();
        let contents = r#"{ "version": 2, "areas": [], "future": true }"#;
        let path = write(&dir, contents);

        let Err(LoadError::UnsupportedVersion(2)) = load(&path) else {
            panic!("A newer version must not load");
        };

        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn tests_never_use_the_real_config_dir() {
        let path = path();
        let dir = std::env::temp_dir().join(format!("kooha-test-{}", std::process::id()));
        assert_eq!(path.parent().unwrap(), dir);
        assert!(!path.starts_with(glib::user_config_dir()));
    }

    #[test]
    fn each_test_store_gets_its_own_file() {
        assert_ne!(path(), path());
    }

    #[test]
    fn selected_is_hashable() {
        let set = std::collections::HashSet::from([
            Selected::Last,
            Selected::Named("Demo".into()),
            Selected::Last,
        ]);
        assert_eq!(set.len(), 2);
    }
}
