use std::cell::Cell;

use gettextrs::gettext;
use gtk::glib;

use crate::recording::RecordingState;

/// What a recording that is about to start will not capture.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "KoohaCaptureWarnings")]
pub struct CaptureWarnings {
    pub missing_desktop_audio: bool,
    pub missing_microphone: bool,
    pub missing_pointer: bool,
}

impl CaptureWarnings {
    pub fn compute(
        profile_supports_audio: bool,
        record_desktop_audio: bool,
        record_microphone: bool,
        show_pointer: bool,
    ) -> Self {
        Self {
            missing_desktop_audio: !profile_supports_audio || !record_desktop_audio,
            missing_microphone: !profile_supports_audio || !record_microphone,
            missing_pointer: !show_pointer,
        }
    }

    pub fn is_empty(self) -> bool {
        !self.missing_desktop_audio && !self.missing_microphone && !self.missing_pointer
    }

    /// Translated labels, audio first.
    pub fn labels(self) -> Vec<String> {
        let mut labels = Vec::new();
        match (self.missing_desktop_audio, self.missing_microphone) {
            (true, true) => labels.push(gettext("Without audio")),
            (true, false) => labels.push(gettext("Without desktop audio")),
            (false, true) => labels.push(gettext("Without microphone")),
            (false, false) => {}
        }
        if self.missing_pointer {
            labels.push(gettext("Without mouse pointer"));
        }
        labels
    }
}

/// Whether to send the capture warnings notification on entering `state`.
pub fn should_notify(
    already_notified: bool,
    state: RecordingState,
    warnings: CaptureWarnings,
) -> bool {
    !already_notified && state == RecordingState::Recording && !warnings.is_empty()
}

/// Sends the capture warnings notification once per recording, when it
/// first enters the recording state.
#[derive(Debug, Default)]
pub struct WarningsNotifier {
    notified: Cell<bool>,
}

impl WarningsNotifier {
    /// Forgets the previous recording.
    pub fn reset(&self) {
        self.notified.set(false);
    }

    pub fn on_state(
        &self,
        state: RecordingState,
        warnings: CaptureWarnings,
        send: impl FnOnce(CaptureWarnings),
    ) {
        if should_notify(self.notified.get(), state, warnings) {
            self.notified.set(true);
            send(warnings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(w: CaptureWarnings) -> Vec<String> {
        w.labels()
    }

    #[test]
    fn nothing_missing() {
        let w = CaptureWarnings::compute(true, true, true, true);
        assert!(w.is_empty());
        assert!(labels(w).is_empty());
    }

    #[test]
    fn only_microphone_missing() {
        let w = CaptureWarnings::compute(true, true, false, true);
        assert_eq!(labels(w), ["Without microphone"]);
    }

    #[test]
    fn only_desktop_audio_missing() {
        let w = CaptureWarnings::compute(true, false, true, true);
        assert_eq!(labels(w), ["Without desktop audio"]);
    }

    #[test]
    fn both_audio_sources_missing() {
        let w = CaptureWarnings::compute(true, false, false, true);
        assert_eq!(labels(w), ["Without audio"]);
    }

    #[test]
    fn profile_without_audio_support() {
        let w = CaptureWarnings::compute(false, true, true, true);
        assert_eq!(labels(w), ["Without audio"]);
    }

    #[test]
    fn pointer_missing_goes_last() {
        let w = CaptureWarnings::compute(true, true, false, false);
        assert_eq!(labels(w), ["Without microphone", "Without mouse pointer"]);
    }

    #[test]
    fn notifies_when_recording_starts_with_warnings() {
        let w = CaptureWarnings::compute(true, true, false, true);
        assert!(should_notify(false, RecordingState::Recording, w));
    }

    #[test]
    fn does_not_notify_twice() {
        let w = CaptureWarnings::compute(true, true, false, true);
        assert!(!should_notify(true, RecordingState::Recording, w));
    }

    #[test]
    fn does_not_notify_during_countdown() {
        let w = CaptureWarnings::compute(true, true, false, true);
        assert!(!should_notify(
            false,
            RecordingState::Delayed { secs_left: 3 },
            w
        ));
    }

    #[test]
    fn does_not_notify_without_warnings() {
        let w = CaptureWarnings::compute(true, true, true, true);
        assert!(!should_notify(false, RecordingState::Recording, w));
    }

    #[test]
    fn notifies_once_per_recording() {
        let w = CaptureWarnings::compute(true, true, false, true);
        let notifier = WarningsNotifier::default();
        let sent = Cell::new(0);
        let states = [
            RecordingState::Delayed { secs_left: 1 },
            RecordingState::Recording,
            RecordingState::Paused,
            RecordingState::Recording,
            RecordingState::Flushing { progress: 0 },
        ];

        for state in states {
            notifier.on_state(state, w, |_| sent.set(sent.get() + 1));
        }
        assert_eq!(sent.get(), 1);

        notifier.reset();
        for state in states {
            notifier.on_state(state, w, |_| sent.set(sent.get() + 1));
        }
        assert_eq!(sent.get(), 2);
    }
}
