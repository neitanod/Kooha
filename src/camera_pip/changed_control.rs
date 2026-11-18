//! The camera control value Kooha changed and has not written back yet, kept
//! in a file while it is changed, so that a Kooha that dies without writing
//! it back does it on its next start.
//!
//! While the control is changed, the Kooha that changed it holds an
//! exclusive `flock` on `camera-control.json.lock`. The kernel releases it
//! when that Kooha dies, so a file whose lock can be taken is a leftover,
//! and one whose lock is held belongs to a Kooha still running.

use std::{
    ffi::OsString,
    fs, io,
    os::fd::AsRawFd,
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

/// A control this Kooha changed and has to write back. It holds the lock of
/// the state file until then, so that another Kooha doesn't take it for a
/// leftover (codex code r2 #3).
#[derive(Debug)]
pub struct HeldControl {
    control: ChangedControl,
    /// `None` if the lock file couldn't be opened.
    _lock: Option<fs::File>,
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

/// Keeps `control` in `path`, then writes 0 to it with `set`. Returns the
/// control to write back if it changed; if it did not, nothing is kept.
///
/// Another Kooha holding the lock has a control changed already: this one
/// changes nothing, as its value would replace the other's.
pub fn change(
    path: &Path,
    control: ChangedControl,
    mut set: impl FnMut(&Path, i32) -> io::Result<()>,
) -> Option<HeldControl> {
    let lock = match try_lock(path) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            tracing::info!(
                "Another Kooha keeps the frame rate of a camera; leaving {} as it is",
                control.node.display()
            );
            return None;
        }
        Err(err) => {
            tracing::warn!("Failed to lock {}: {:?}", path.display(), err);
            None
        }
    };

    // Kept first: Kooha may die right after writing 0.
    if let Err(err) = save(path, &control) {
        tracing::warn!(
            "Failed to keep the camera control in {}: {:?}",
            path.display(),
            err
        );
    }

    match set(&control.node, 0) {
        Ok(()) => Some(HeldControl {
            control,
            _lock: lock,
        }),
        Err(err) => {
            tracing::warn!(
                "Failed to keep the frame rate of {}: {:?}",
                control.node.display(),
                err
            );
            forget(path);
            None
        }
    }
}

/// Writes `held`'s value back with `set` and forgets it, also when the
/// write fails: a camera that is gone may leave its node to another one.
/// The lock goes last.
pub fn write_back(
    path: &Path,
    held: HeldControl,
    mut set: impl FnMut(&Path, i32) -> io::Result<()>,
) {
    let control = &held.control;
    if let Err(err) = set(&control.node, control.value) {
        tracing::warn!(
            "Failed to restore the frame rate setting of {}: {:?}",
            control.node.display(),
            err
        );
    }
    forget(path);
    drop(held);
}

/// Writes back the control that a previous Kooha left changed, if any. A
/// control whose lock another Kooha holds is that Kooha's, still running,
/// and is left alone.
pub fn restore_leftover(path: &Path, mut set: impl FnMut(&Path, i32) -> io::Result<()>) {
    if !path.exists() {
        return;
    }

    // An error says nothing about an owner: restore, as before the lock.
    let lock = match try_lock(path) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            tracing::debug!(
                "The camera control in {} belongs to a running Kooha",
                path.display()
            );
            return;
        }
        Err(err) => {
            tracing::warn!("Failed to lock {}: {:?}", path.display(), err);
            None
        }
    };

    match load(path) {
        Some(control) if v4l2::video_node(&control.node.to_string_lossy()).is_some() => {
            tracing::info!(
                node = %control.node.display(),
                value = control.value,
                "Restoring a camera control left changed"
            );
            write_back(
                path,
                HeldControl {
                    control,
                    _lock: lock,
                },
                &mut set,
            );
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

/// Takes the lock of the state file at `path` without waiting: `None` if
/// another Kooha, or another handle of this one, holds it.
fn try_lock(path: &Path) -> io::Result<Option<fs::File>> {
    let lock_path = lock_path(path);
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    loop {
        // SAFETY: the descriptor is open for as long as `file` lives.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(file));
        }
        let err = io::Error::last_os_error();
        match err.kind() {
            io::ErrorKind::WouldBlock => return Ok(None),
            io::ErrorKind::Interrupted => {}
            _ => return Err(err),
        }
    }
}

/// `camera-control.json.lock`, next to the state file. The file itself
/// can't hold the lock: every save renames a new one into place.
fn lock_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".lock");
    PathBuf::from(name)
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
    fs::write(
        &temp,
        serde_json::to_vec(control).map_err(io::Error::other)?,
    )?;
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

    /// A state file path in a directory of its own, which does not exist
    /// yet and is removed on drop (fable code r2 #7).
    struct TempState {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempState {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kooha-changed-control-test-{}-{name}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            let path = dir.join("kooha").join("camera-control.json");
            Self { dir, path }
        }
    }

    impl Drop for TempState {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
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
        let state = TempState::new("kept");
        let path = &state.path;
        let calls = RefCell::new(Vec::new());

        let held = change(
            path,
            control("/dev/video0", 1),
            recording_setter(&calls, || Ok(())),
        )
        .unwrap();
        assert_eq!(*calls.borrow(), [(PathBuf::from("/dev/video0"), 0)]);
        assert_eq!(load(path), Some(control("/dev/video0", 1)));

        write_back(path, held, recording_setter(&calls, || Ok(())));
        assert_eq!(calls.borrow()[1], (PathBuf::from("/dev/video0"), 1));
        assert!(!path.exists());
    }

    #[test]
    fn a_control_that_could_not_change_is_not_kept() {
        let state = TempState::new("not-changed");
        let path = &state.path;
        let calls = RefCell::new(Vec::new());

        let held = change(
            path,
            control("/dev/video0", 1),
            recording_setter(&calls, || Err(io::Error::other("busy"))),
        );
        assert!(held.is_none());
        assert!(!path.exists());
    }

    #[test]
    fn a_failed_write_back_forgets_the_control() {
        let state = TempState::new("failed-write-back");
        let path = &state.path;
        let calls = RefCell::new(Vec::new());
        let held = change(
            path,
            control("/dev/video0", 1),
            recording_setter(&calls, || Ok(())),
        )
        .unwrap();

        // The camera is gone: another one may get its node.
        write_back(
            path,
            held,
            recording_setter(&calls, || Err(io::Error::other("unplugged"))),
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_leftover_control_is_written_back_at_start() {
        let state = TempState::new("leftover");
        let path = &state.path;
        save(path, &control("/dev/video2", 1)).unwrap();
        let calls = RefCell::new(Vec::new());

        restore_leftover(path, recording_setter(&calls, || Ok(())));

        assert_eq!(*calls.borrow(), [(PathBuf::from("/dev/video2"), 1)]);
        assert!(!path.exists());
    }

    #[test]
    fn a_control_another_live_kooha_changed_is_left_alone() {
        let state = TempState::new("live-owner");
        let path = &state.path;
        let calls = RefCell::new(Vec::new());
        let held = change(
            path,
            control("/dev/video0", 1),
            recording_setter(&calls, || Ok(())),
        )
        .unwrap();

        // A second Kooha starts while the first one records with the
        // camera on (codex code r2 #3).
        let other_calls = RefCell::new(Vec::new());
        restore_leftover(path, recording_setter(&other_calls, || Ok(())));
        assert!(other_calls.borrow().is_empty());
        assert_eq!(load(path), Some(control("/dev/video0", 1)));

        // Nor does it change the control under the first one.
        let other = change(
            path,
            control("/dev/video0", 0),
            recording_setter(&other_calls, || Ok(())),
        );
        assert!(other.is_none());
        assert!(other_calls.borrow().is_empty());
        assert_eq!(load(path), Some(control("/dev/video0", 1)));

        write_back(path, held, recording_setter(&calls, || Ok(())));
        assert_eq!(calls.borrow()[1], (PathBuf::from("/dev/video0"), 1));
        assert!(!path.exists());
    }

    #[test]
    fn a_control_left_by_a_kooha_that_died_is_restored() {
        let state = TempState::new("dead-owner");
        let path = &state.path;
        let held = change(path, control("/dev/video0", 1), |_, _| Ok(())).unwrap();
        // Dying releases the lock, and writes nothing back.
        drop(held);

        let calls = RefCell::new(Vec::new());
        restore_leftover(path, recording_setter(&calls, || Ok(())));

        assert_eq!(*calls.borrow(), [(PathBuf::from("/dev/video0"), 1)]);
        assert!(!path.exists());
    }

    #[test]
    fn the_test_state_is_removed_on_drop() {
        let state = TempState::new("removed");
        save(&state.path, &control("/dev/video0", 1)).unwrap();
        let dir = state.dir.clone();
        assert!(dir.exists());

        drop(state);

        assert!(!dir.exists());
    }

    #[test]
    fn no_leftover_writes_nothing() {
        let state = TempState::new("none");
        let path = &state.path;
        let calls = RefCell::new(Vec::new());

        restore_leftover(path, recording_setter(&calls, || Ok(())));

        assert!(calls.borrow().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn a_broken_or_foreign_leftover_is_dropped_unwritten() {
        for (name, contents) in [
            ("broken", r#"{"node": "/dev/video0", "val"#),
            ("not-a-video-node", r#"{"node": "/dev/sda", "value": 1}"#),
        ] {
            let state = TempState::new(name);
            let path = &state.path;
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
            let calls = RefCell::new(Vec::new());

            restore_leftover(path, recording_setter(&calls, || Ok(())));

            assert!(calls.borrow().is_empty(), "{name}");
            assert!(!path.exists(), "{name}");
        }
    }
}
