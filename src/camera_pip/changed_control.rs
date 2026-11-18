//! The camera control value Kooha changed and has not written back yet, kept
//! in a file while it is changed, so that a Kooha that dies without writing
//! it back does it on its next start.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::v4l2;

/// The value `exposure_dynamic_framerate` had on `node` before Kooha wrote 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedControl {
    pub node: PathBuf,
    pub value: i32,
}

/// Where the changed control is kept: the user state directory, or a
/// temporary directory in tests.
pub fn default_path() -> PathBuf {
    #[cfg(not(test))]
    let dir = gtk::glib::user_state_dir();
    #[cfg(test)]
    let dir = std::env::temp_dir().join(format!("kooha-test-state-{}", std::process::id()));
    dir.join("kooha").join("camera-control.json")
}

/// Keeps `control` in `path`, then writes 0 to it with `set`. Returns
/// whether it changed; if it did not, nothing is kept.
pub fn change(
    path: &Path,
    control: &ChangedControl,
    mut set: impl FnMut(&Path, i32) -> io::Result<()>,
) -> bool {
    // Kept first: Kooha may die right after writing 0.
    if let Err(err) = save(path, control) {
        tracing::warn!(
            "Failed to keep the camera control in {}: {:?}",
            path.display(),
            err
        );
    }

    match set(&control.node, 0) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(
                "Failed to keep the frame rate of {}: {:?}",
                control.node.display(),
                err
            );
            forget(path);
            false
        }
    }
}

/// Writes `control`'s value back with `set` and forgets it, also when the
/// write fails: a camera that is gone may leave its node to another one.
pub fn write_back(
    path: &Path,
    control: &ChangedControl,
    mut set: impl FnMut(&Path, i32) -> io::Result<()>,
) {
    if let Err(err) = set(&control.node, control.value) {
        tracing::warn!(
            "Failed to restore the frame rate setting of {}: {:?}",
            control.node.display(),
            err
        );
    }
    forget(path);
}

/// Writes back the control that a previous Kooha left changed, if any.
pub fn restore_leftover(path: &Path, set: impl FnMut(&Path, i32) -> io::Result<()>) {
    if !path.exists() {
        return;
    }

    match load(path) {
        Some(control) if v4l2::video_node(&control.node.to_string_lossy()).is_some() => {
            tracing::info!(
                node = %control.node.display(),
                value = control.value,
                "Restoring a camera control left changed"
            );
            write_back(path, &control, set);
        }
        other => {
            tracing::warn!(
                "Dropping the camera control state in {}: {:?}",
                path.display(),
                other
            );
            forget(path);
        }
    }
}

/// The control kept in `path`, if it holds a valid one.
fn load(path: &Path) -> Option<ChangedControl> {
    let contents = fs::read(path).ok()?;
    serde_json::from_slice(&contents)
        .inspect_err(|err| tracing::warn!("Invalid {}: {:?}", path.display(), err))
        .ok()
}

/// Writes `control` to `path` through a temporary file, so that `path` is
/// never left half written.
fn save(path: &Path, control: &ChangedControl) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec(control).map_err(io::Error::other)?)?;
    fs::rename(&temp, path)
}

fn forget(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!("Failed to remove {}: {:?}", path.display(), err),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, fs, io};

    use super::*;

    /// A state file path in a directory of its own, which does not exist yet.
    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kooha-changed-control-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir.join("kooha").join("camera-control.json")
    }

    /// A setter that records its calls and answers `answer`.
    fn recording_setter(
        calls: &RefCell<Vec<(PathBuf, i32)>>,
        answer: fn() -> io::Result<()>,
    ) -> impl FnMut(&Path, i32) -> io::Result<()> + '_ {
        move |node, value| {
            calls.borrow_mut().push((node.to_path_buf(), value));
            answer()
        }
    }

    fn control(node: &str, value: i32) -> ChangedControl {
        ChangedControl {
            node: PathBuf::from(node),
            value,
        }
    }

    #[test]
    fn the_test_state_file_is_not_the_user_one() {
        let path = default_path();
        assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
        assert!(!path.starts_with(gtk::glib::user_state_dir()));
        assert_eq!(path.file_name().unwrap(), "camera-control.json");
    }

    #[test]
    fn a_changed_control_is_kept_until_written_back() {
        let path = temp_path("kept");
        let calls = RefCell::new(Vec::new());

        let changed = change(
            &path,
            &control("/dev/video0", 1),
            recording_setter(&calls, || Ok(())),
        );
        assert!(changed);
        assert_eq!(*calls.borrow(), [(PathBuf::from("/dev/video0"), 0)]);
        assert_eq!(load(&path), Some(control("/dev/video0", 1)));

        write_back(
            &path,
            &control("/dev/video0", 1),
            recording_setter(&calls, || Ok(())),
        );
        assert_eq!(calls.borrow()[1], (PathBuf::from("/dev/video0"), 1));
        assert!(!path.exists());
    }

    #[test]
    fn a_control_that_could_not_change_is_not_kept() {
        let path = temp_path("not-changed");
        let calls = RefCell::new(Vec::new());

        let changed = change(
            &path,
            &control("/dev/video0", 1),
            recording_setter(&calls, || Err(io::Error::other("busy"))),
        );
        assert!(!changed);
        assert!(!path.exists());
    }

    #[test]
    fn a_failed_write_back_forgets_the_control() {
        let path = temp_path("failed-write-back");
        let calls = RefCell::new(Vec::new());
        assert!(change(
            &path,
            &control("/dev/video0", 1),
            recording_setter(&calls, || Ok(())),
        ));

        // The camera is gone: another one may get its node.
        write_back(
            &path,
            &control("/dev/video0", 1),
            recording_setter(&calls, || Err(io::Error::other("unplugged"))),
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_leftover_control_is_written_back_at_start() {
        let path = temp_path("leftover");
        save(&path, &control("/dev/video2", 1)).unwrap();
        let calls = RefCell::new(Vec::new());

        restore_leftover(&path, recording_setter(&calls, || Ok(())));

        assert_eq!(*calls.borrow(), [(PathBuf::from("/dev/video2"), 1)]);
        assert!(!path.exists());
    }

    #[test]
    fn no_leftover_writes_nothing() {
        let path = temp_path("none");
        let calls = RefCell::new(Vec::new());

        restore_leftover(&path, recording_setter(&calls, || Ok(())));

        assert!(calls.borrow().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn a_broken_or_foreign_leftover_is_dropped_unwritten() {
        for (name, contents) in [
            ("broken", r#"{"node": "/dev/video0", "val"#),
            ("not-a-video-node", r#"{"node": "/dev/sda", "value": 1}"#),
        ] {
            let path = temp_path(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            let calls = RefCell::new(Vec::new());

            restore_leftover(&path, recording_setter(&calls, || Ok(())));

            assert!(calls.borrow().is_empty(), "{name}");
            assert!(!path.exists(), "{name}");
        }
    }
}
