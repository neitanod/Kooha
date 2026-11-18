//! What the main window shows of the saved areas: the dropdown to choose
//! one and the notices of the store as toasts.

use gettextrs::gettext;

use crate::{
    areas::{AreaStore, MarkedArea, MonitorInfo, SavedArea, Selected, file::LoadError},
    i18n::gettext_f,
    recording::RecordingState,
};

/// The entries of the areas dropdown, in order: each saved area as
/// `name · 1280×720 on DP-3`, then the last marked area if there is one.
/// Each comes with what choosing it selects and whether it is out of date,
/// which puts a warning sign before it.
pub fn dropdown_items(
    areas: &[SavedArea],
    last: Option<&MarkedArea>,
    monitors: &[MonitorInfo],
) -> Vec<(String, Selected, bool)> {
    let item = |label: String, selected: Selected, area: &MarkedArea| {
        let stale = area.staleness(monitors).is_some();
        let label = if stale { format!("⚠ {label}") } else { label };
        (label, selected, stale)
    };

    let saved = areas.iter().map(|saved| {
        item(
            format!("{} · {}", saved.name, saved.area.describe()),
            Selected::Named(saved.name.clone()),
            &saved.area,
        )
    });
    let last = last.map(|area| {
        item(
            gettext_f("Last area · {area}", &[("area", &area.describe())]),
            Selected::Last,
            area,
        )
    });
    saved.chain(last).collect()
}

/// Which window actions are enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnabledActions {
    /// `win.toggle-record`.
    pub toggle_record: bool,
    /// `win.mark-area` and `win.remark-area`.
    pub mark_area: bool,
}

/// The actions enabled in the recording `state`, while an area is being
/// marked or not. Marking needs no recording in course; recording waits
/// for the marking to end.
pub fn enabled_actions(state: RecordingState, marking: bool) -> EnabledActions {
    let idle = matches!(state, RecordingState::Init | RecordingState::Finished);
    let busy = matches!(
        state,
        RecordingState::Delayed { .. } | RecordingState::Flushing { .. }
    );
    EnabledActions {
        toggle_record: !marking && !busy,
        mark_area: !marking && idle,
    }
}

/// The toast for an areas file that couldn't be loaded.
pub fn notice_for_load_error(err: &LoadError) -> String {
    match err {
        LoadError::Broken { moved_to } => gettext_f(
            "The list of areas was unreadable; it was moved to {path}",
            &[("path", &moved_to.display().to_string())],
        ),
        LoadError::Io(err) => gettext_f(
            "Couldn't read the list of areas: {error}",
            &[("error", &err.to_string())],
        ),
        LoadError::UnsupportedVersion(_) => {
            gettext("The list of areas was written by a newer Kooha; changes won't be saved")
        }
    }
}

/// The toast for why the store started without the saved list, once: the
/// next call returns `None`.
pub fn notice_for_store(store: &AreaStore) -> Option<String> {
    store.load_notice().map(|err| notice_for_load_error(&err))
}

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use super::*;
    use crate::areas::{
        file::test_utils::TempDir,
        model::test_monitors::{dp3, edp},
    };

    fn area(
        connector: &str,
        width: i32,
        height: i32,
        monitor: (i32, i32),
        scale: f64,
    ) -> MarkedArea {
        MarkedArea {
            connector: connector.into(),
            x: 0,
            y: 0,
            width,
            height,
            monitor_width: monitor.0,
            monitor_height: monitor.1,
            scale,
        }
    }

    fn saved(name: &str, area: MarkedArea) -> SavedArea {
        SavedArea {
            name: name.into(),
            area,
        }
    }

    #[test]
    fn dropdown_lists_the_saved_areas_then_the_last_one() {
        let monitors = [edp(), dp3()];
        let areas = [
            saved("Demo", area("DP-3", 1280, 720, (1680, 1050), 1.05)),
            saved("Panel", area("eDP-1", 640, 480, (2560, 1600), 1.75)),
        ];
        let last = area("DP-3", 800, 600, (1680, 1050), 1.05);

        assert_eq!(
            dropdown_items(&areas, Some(&last), &monitors),
            [
                (
                    "Demo · 1280×720 on DP-3".to_string(),
                    Selected::Named("Demo".into()),
                    false
                ),
                (
                    "Panel · 640×480 on eDP-1".to_string(),
                    Selected::Named("Panel".into()),
                    false
                ),
                (
                    "Last area · 800×600 on DP-3".to_string(),
                    Selected::Last,
                    false
                ),
            ]
        );
    }

    #[test]
    fn dropdown_warns_of_out_of_date_areas() {
        // DP-3 disconnected, and the panel at another scale.
        let monitors = [edp()];
        let areas = [
            saved("Demo", area("DP-3", 1280, 720, (1680, 1050), 1.05)),
            saved("Panel", area("eDP-1", 640, 480, (2560, 1600), 1.5)),
        ];
        let last = area("DP-3", 800, 600, (1680, 1050), 1.05);

        assert_eq!(
            dropdown_items(&areas, Some(&last), &monitors),
            [
                (
                    "⚠ Demo · 1280×720 on DP-3".to_string(),
                    Selected::Named("Demo".into()),
                    true
                ),
                (
                    "⚠ Panel · 640×480 on eDP-1".to_string(),
                    Selected::Named("Panel".into()),
                    true
                ),
                (
                    "⚠ Last area · 800×600 on DP-3".to_string(),
                    Selected::Last,
                    true
                ),
            ]
        );
    }

    #[test]
    fn dropdown_is_empty_without_areas() {
        assert!(dropdown_items(&[], None, &[edp()]).is_empty());
    }

    #[test]
    fn actions_by_recording_state_and_marking() {
        let enabled = |toggle_record, mark_area| EnabledActions {
            toggle_record,
            mark_area,
        };
        let table = [
            (RecordingState::Init, false, enabled(true, true)),
            (RecordingState::Init, true, enabled(false, false)),
            (RecordingState::Finished, false, enabled(true, true)),
            (RecordingState::Finished, true, enabled(false, false)),
            (
                RecordingState::Delayed { secs_left: 3 },
                false,
                enabled(false, false),
            ),
            (RecordingState::Recording, false, enabled(true, false)),
            (RecordingState::Recording, true, enabled(false, false)),
            (RecordingState::Paused, false, enabled(true, false)),
            (
                RecordingState::Flushing { progress: 50 },
                false,
                enabled(false, false),
            ),
        ];
        for (state, marking, expected) in table {
            assert_eq!(
                enabled_actions(state, marking),
                expected,
                "{state:?}, marking: {marking}"
            );
        }
    }

    #[test]
    fn broken_file_is_noticed_once_with_where_it_went() {
        let dir = TempDir::new();
        let path = dir.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();

        let store = AreaStore::with_path(path);

        let notice = notice_for_store(&store).expect("a broken file is noticed");
        assert!(
            notice.starts_with("The list of areas was unreadable; it was moved to "),
            "{notice}"
        );
        assert!(notice.contains("rectangles.json.broken-"), "{notice}");
        assert_eq!(notice_for_store(&store), None);
    }

    #[test]
    fn readable_file_has_no_notice() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());

        assert_eq!(notice_for_store(&store), None);
    }

    #[test]
    fn broken_file_notice_says_where_it_went() {
        let notice = notice_for_load_error(&LoadError::Broken {
            moved_to: "/tmp/rectangles.json.broken-1".into(),
        });
        assert_eq!(
            notice,
            "The list of areas was unreadable; it was moved to /tmp/rectangles.json.broken-1"
        );
    }

    #[test]
    fn unreadable_file_notice_has_the_error() {
        assert_eq!(
            notice_for_load_error(&LoadError::Io(io::Error::other("denied"))),
            "Couldn't read the list of areas: denied"
        );
    }

    #[test]
    fn newer_file_notice_says_changes_are_not_saved() {
        assert_eq!(
            notice_for_load_error(&LoadError::UnsupportedVersion(2)),
            "The list of areas was written by a newer Kooha; changes won't be saved"
        );
    }
}
