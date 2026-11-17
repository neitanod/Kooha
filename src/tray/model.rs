use gettextrs::gettext;

use crate::{config::APP_ID, i18n::gettext_f, recording::RecordingState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// `Starting` covers a recording that exists but has not begun yet: the portal
/// dialog, the area selector and the countdown.
pub enum TrayRecordingState {
    Idle,
    Starting,
    Recording,
    Paused,
    Flushing,
}

/// A snapshot of what the tray shows.
// The bools mirror independent settings keys, not a hidden state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq)]
pub struct TrayModel {
    pub state: TrayRecordingState,
    /// The recording duration, already formatted.
    pub duration_text: String,
    /// `CaptureWarnings::labels()` of the current recording.
    pub warnings: Vec<String>,
    pub record_desktop_audio: bool,
    pub record_microphone: bool,
    pub show_pointer: bool,
    /// Whether the active profile supports audio.
    pub audio_supported: bool,
    pub camera_enabled: bool,
    /// Whether overlays are available to show the camera.
    pub camera_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ToggleRecord,
    TogglePause,
    ToggleDesktopAudio,
    ToggleMicrophone,
    TogglePointer,
    ToggleCamera,
    ShowWindow,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrayEntry {
    /// Not clickable.
    Label(String),
    Button {
        label: String,
        command: TrayCommand,
        enabled: bool,
    },
    Check {
        label: String,
        checked: bool,
        command: TrayCommand,
        enabled: bool,
    },
    Separator,
}

/// The tray state for the window's recording, `None` when there is none.
pub fn tray_state(state: Option<RecordingState>) -> TrayRecordingState {
    match state {
        None | Some(RecordingState::Finished) => TrayRecordingState::Idle,
        Some(RecordingState::Init | RecordingState::Delayed { .. }) => TrayRecordingState::Starting,
        Some(RecordingState::Recording) => TrayRecordingState::Recording,
        Some(RecordingState::Paused) => TrayRecordingState::Paused,
        Some(RecordingState::Flushing { .. }) => TrayRecordingState::Flushing,
    }
}

/// The menu for `model`, top to bottom.
pub fn menu_entries(model: &TrayModel) -> Vec<TrayEntry> {
    use TrayRecordingState::{Idle, Paused, Recording};

    let mut entries = vec![TrayEntry::Label(status_text(model))];
    entries.extend(model.warnings.iter().cloned().map(TrayEntry::Label));
    entries.push(TrayEntry::Separator);

    entries.push(TrayEntry::Button {
        label: if model.state == Idle {
            gettext("Start Recording")
        } else {
            gettext("Stop Recording")
        },
        command: TrayCommand::ToggleRecord,
        // Same as `win.toggle-record`, which is disabled during the
        // countdown and while saving.
        enabled: matches!(model.state, Idle | Recording | Paused),
    });
    // Same states as `win.toggle-pause`.
    if matches!(model.state, Recording | Paused) {
        entries.push(TrayEntry::Button {
            label: if model.state == Paused {
                gettext("Resume")
            } else {
                gettext("Pause")
            },
            command: TrayCommand::TogglePause,
            enabled: true,
        });
    }
    entries.push(TrayEntry::Separator);

    // A recording reads these settings only once, when it begins.
    let is_idle = model.state == Idle;
    let audio_enabled = is_idle && model.audio_supported;
    entries.push(TrayEntry::Check {
        label: gettext("Record Desktop Audio"),
        checked: model.record_desktop_audio,
        command: TrayCommand::ToggleDesktopAudio,
        enabled: audio_enabled,
    });
    entries.push(TrayEntry::Check {
        label: gettext("Record Microphone"),
        checked: model.record_microphone,
        command: TrayCommand::ToggleMicrophone,
        enabled: audio_enabled,
    });
    entries.push(TrayEntry::Check {
        label: gettext("Show Pointer"),
        checked: model.show_pointer,
        command: TrayCommand::TogglePointer,
        enabled: is_idle,
    });
    // The camera is on screen, so it can change at any time.
    entries.push(TrayEntry::Check {
        label: gettext("Camera"),
        checked: model.camera_enabled,
        command: TrayCommand::ToggleCamera,
        enabled: model.camera_available,
    });
    entries.push(TrayEntry::Separator);

    entries.push(TrayEntry::Button {
        label: gettext("Show Window"),
        command: TrayCommand::ShowWindow,
        enabled: true,
    });
    entries.push(TrayEntry::Button {
        label: gettext("Quit"),
        command: TrayCommand::Quit,
        enabled: true,
    });

    entries
}

pub fn icon_name(state: TrayRecordingState) -> &'static str {
    match state {
        TrayRecordingState::Idle | TrayRecordingState::Starting => APP_ID,
        TrayRecordingState::Recording => "media-record-symbolic",
        TrayRecordingState::Paused => "media-playback-pause-symbolic",
        TrayRecordingState::Flushing => "document-save-symbolic",
    }
}

/// The tooltip's title and body.
pub fn tooltip(model: &TrayModel) -> (String, String) {
    let mut lines = vec![status_text(model)];
    lines.extend(model.warnings.iter().cloned());

    (gettext("Kooha"), lines.join("\n"))
}

fn status_text(model: &TrayModel) -> String {
    match model.state {
        TrayRecordingState::Idle => gettext("Ready"),
        TrayRecordingState::Starting => gettext("Starting…"),
        TrayRecordingState::Recording => {
            gettext_f("Recording — {time}", &[("time", &model.duration_text)])
        }
        TrayRecordingState::Paused => {
            gettext_f("Paused — {time}", &[("time", &model.duration_text)])
        }
        TrayRecordingState::Flushing => gettext("Saving…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(state: TrayRecordingState) -> TrayModel {
        TrayModel {
            state,
            duration_text: "00:12".to_string(),
            warnings: Vec::new(),
            record_desktop_audio: true,
            record_microphone: false,
            show_pointer: true,
            audio_supported: true,
            camera_enabled: false,
            camera_available: true,
        }
    }

    fn checks(entries: &[TrayEntry]) -> Vec<(TrayCommand, bool, bool)> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                TrayEntry::Check {
                    command,
                    checked,
                    enabled,
                    ..
                } => Some((*command, *checked, *enabled)),
                _ => None,
            })
            .collect()
    }

    fn record_button(entries: &[TrayEntry]) -> (String, bool) {
        entries
            .iter()
            .find_map(|entry| match entry {
                TrayEntry::Button {
                    label,
                    command: TrayCommand::ToggleRecord,
                    enabled,
                } => Some((label.clone(), *enabled)),
                _ => None,
            })
            .expect("menu has a record button")
    }

    fn button(label: &str, command: TrayCommand) -> TrayEntry {
        TrayEntry::Button {
            label: label.to_string(),
            command,
            enabled: true,
        }
    }

    #[test]
    fn idle_menu() {
        let entries = menu_entries(&model(TrayRecordingState::Idle));

        assert_eq!(entries[0], TrayEntry::Label("Ready".to_string()));
        assert!(entries.contains(&button("Start Recording", TrayCommand::ToggleRecord)));
        assert_eq!(
            checks(&entries),
            [
                (TrayCommand::ToggleDesktopAudio, true, true),
                (TrayCommand::ToggleMicrophone, false, true),
                (TrayCommand::TogglePointer, true, true),
                (TrayCommand::ToggleCamera, false, true),
            ]
        );
        assert_eq!(
            entries[entries.len() - 2..],
            [
                button("Show Window", TrayCommand::ShowWindow),
                button("Quit", TrayCommand::Quit),
            ]
        );
    }

    #[test]
    fn recording_menu_disables_capture_toggles() {
        let mut model = model(TrayRecordingState::Recording);
        model.warnings = vec![
            "Without microphone".to_string(),
            "Without mouse pointer".to_string(),
        ];

        let entries = menu_entries(&model);

        assert_eq!(
            entries[..3],
            [
                TrayEntry::Label("Recording — 00:12".to_string()),
                TrayEntry::Label("Without microphone".to_string()),
                TrayEntry::Label("Without mouse pointer".to_string()),
            ]
        );
        assert!(entries.contains(&button("Stop Recording", TrayCommand::ToggleRecord)));
        assert_eq!(
            checks(&entries),
            [
                (TrayCommand::ToggleDesktopAudio, true, false),
                (TrayCommand::ToggleMicrophone, false, false),
                (TrayCommand::TogglePointer, true, false),
                (TrayCommand::ToggleCamera, false, true),
            ]
        );
    }

    #[test]
    fn paused_menu_shows_time() {
        let entries = menu_entries(&model(TrayRecordingState::Paused));

        assert_eq!(entries[0], TrayEntry::Label("Paused — 00:12".to_string()));
        assert_eq!(
            record_button(&entries),
            ("Stop Recording".to_string(), true)
        );
    }

    #[test]
    fn starting_and_flushing_disable_toggles() {
        for (state, status) in [
            (TrayRecordingState::Starting, "Starting…"),
            (TrayRecordingState::Flushing, "Saving…"),
        ] {
            let entries = menu_entries(&model(state));

            assert_eq!(entries[0], TrayEntry::Label(status.to_string()));
            assert!(!record_button(&entries).1, "{state:?}");
            assert!(
                checks(&entries)
                    .iter()
                    .filter(|(command, _, _)| *command != TrayCommand::ToggleCamera)
                    .all(|(_, _, enabled)| !enabled),
                "{state:?}"
            );
        }
    }

    #[test]
    fn audio_checks_disabled_without_audio_support() {
        let mut model = model(TrayRecordingState::Idle);
        model.audio_supported = false;

        assert_eq!(
            checks(&menu_entries(&model)),
            [
                (TrayCommand::ToggleDesktopAudio, true, false),
                (TrayCommand::ToggleMicrophone, false, false),
                (TrayCommand::TogglePointer, true, true),
                (TrayCommand::ToggleCamera, false, true),
            ]
        );
    }

    #[test]
    fn icon_follows_state() {
        assert_eq!(icon_name(TrayRecordingState::Idle), crate::config::APP_ID);
        assert_eq!(
            icon_name(TrayRecordingState::Starting),
            crate::config::APP_ID
        );
        assert_eq!(
            icon_name(TrayRecordingState::Flushing),
            "document-save-symbolic"
        );
        assert_eq!(
            icon_name(TrayRecordingState::Recording),
            "media-record-symbolic"
        );
        assert_eq!(
            icon_name(TrayRecordingState::Paused),
            "media-playback-pause-symbolic"
        );
    }

    #[test]
    fn tray_state_from_recording_state() {
        use crate::recording::RecordingState;

        let cases = [
            (None, TrayRecordingState::Idle),
            (Some(RecordingState::Init), TrayRecordingState::Starting),
            (
                Some(RecordingState::Delayed { secs_left: 3 }),
                TrayRecordingState::Starting,
            ),
            (
                Some(RecordingState::Recording),
                TrayRecordingState::Recording,
            ),
            (Some(RecordingState::Paused), TrayRecordingState::Paused),
            (
                Some(RecordingState::Flushing { progress: 40 }),
                TrayRecordingState::Flushing,
            ),
            (Some(RecordingState::Finished), TrayRecordingState::Idle),
        ];

        for (state, expected) in cases {
            assert_eq!(tray_state(state), expected, "{state:?}");
        }
    }

    #[test]
    fn tooltip_lists_warnings() {
        let mut model = model(TrayRecordingState::Recording);
        model.warnings = vec![
            "Without microphone".to_string(),
            "Without mouse pointer".to_string(),
        ];

        let (title, body) = tooltip(&model);

        assert_eq!(title, "Kooha");
        assert!(body.contains("Recording — 00:12"), "{body}");
        assert!(body.contains("Without microphone"), "{body}");
        assert!(body.contains("Without mouse pointer"), "{body}");
    }

    fn entry_after_record_button(entries: &[TrayEntry]) -> &TrayEntry {
        let index = entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    TrayEntry::Button {
                        command: TrayCommand::ToggleRecord,
                        ..
                    }
                )
            })
            .expect("menu has a record button");
        &entries[index + 1]
    }

    fn has_pause_button(entries: &[TrayEntry]) -> bool {
        entries.iter().any(|entry| {
            matches!(
                entry,
                TrayEntry::Button {
                    command: TrayCommand::TogglePause,
                    ..
                }
            )
        })
    }

    #[test]
    fn pause_button_follows_state() {
        let recording = menu_entries(&model(TrayRecordingState::Recording));
        assert_eq!(
            entry_after_record_button(&recording),
            &button("Pause", TrayCommand::TogglePause)
        );

        let paused = menu_entries(&model(TrayRecordingState::Paused));
        assert_eq!(
            entry_after_record_button(&paused),
            &button("Resume", TrayCommand::TogglePause)
        );

        for state in [
            TrayRecordingState::Idle,
            TrayRecordingState::Starting,
            TrayRecordingState::Flushing,
        ] {
            assert!(!has_pause_button(&menu_entries(&model(state))), "{state:?}");
        }
    }

    fn position(entries: &[TrayEntry], wanted: impl Fn(&TrayEntry) -> bool) -> usize {
        entries.iter().position(wanted).expect("menu has the entry")
    }

    #[test]
    fn camera_check() {
        let mut model = model(TrayRecordingState::Recording);
        model.camera_enabled = true;

        let entries = menu_entries(&model);
        let camera = position(&entries, |entry| {
            matches!(
                entry,
                TrayEntry::Check {
                    command: TrayCommand::ToggleCamera,
                    ..
                }
            )
        });
        let show_window = position(&entries, |entry| {
            matches!(
                entry,
                TrayEntry::Button {
                    command: TrayCommand::ShowWindow,
                    ..
                }
            )
        });

        assert!(camera < show_window);
        // Enabled even while recording.
        assert_eq!(
            entries[camera],
            TrayEntry::Check {
                label: "Camera".to_string(),
                checked: true,
                command: TrayCommand::ToggleCamera,
                enabled: true,
            }
        );

        model.camera_available = false;
        assert!(checks(&menu_entries(&model)).contains(&(TrayCommand::ToggleCamera, true, false)));
    }
}
