//! The eight checks of the requirements report. Each one receives what it
//! queried, so the decision is testable without the real system.

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use gettextrs::gettext;

use super::{CheckResult, Failure, Status};
use crate::{
    i18n::{gettext_f, ngettext_f},
    overlay::OverlayUnavailable,
    screencast_portal::CursorMode,
};

/// The elements of a recording profile, as `gstreamer_elements` needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileElements {
    pub id: String,
    pub name: String,
    pub experimental: bool,
    pub elements: Vec<String>,
}

/// The elements the recording pipeline creates itself, besides the profile's.
pub const PIPELINE_ELEMENTS: [&str; 15] = [
    "pipewiresrc",
    "pulsesrc",
    "videoconvert",
    "videoscale",
    "videocrop",
    "videobalance",
    "videoflip",
    "videorate",
    "compositor",
    "capsfilter",
    "queue",
    "identity",
    "audiomixer",
    "audiorate",
    "filesink",
];

/// The elements the camera `PiP` adds to `videoconvert`: `decodebin`, and
/// what it plugs for a JPEG camera (fable code r1 #2).
pub const CAMERA_ELEMENTS: [&str; 3] = ["decodebin", "jpegparse", "jpegdec"];

/// Where an element comes from on Ubuntu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ElementSource {
    Package(&'static str),
    /// Registered statically by `main.rs`.
    Bundled,
}

/// The Ubuntu package of each element, checked against the `GStreamer` registry
/// and `dpkg -S` on each plugin file. The order is the order of the command.
const PACKAGES: [(&str, &[&str]); 7] = [
    (
        "libgstreamer1.0-0",
        &["queue", "capsfilter", "identity", "filesink"],
    ),
    ("gstreamer1.0-pipewire", &["pipewiresrc"]),
    (
        "gstreamer1.0-plugins-base",
        &[
            "videoconvert",
            "videoscale",
            "videorate",
            "compositor",
            "audioconvert",
            "audioresample",
            "audiomixer",
            "audiorate",
            "opusenc",
            "decodebin",
        ],
    ),
    (
        "gstreamer1.0-plugins-good",
        &[
            "pulsesrc",
            "videocrop",
            "videobalance",
            "videoflip",
            "v4l2src",
            "vp8enc",
            "vp9enc",
            "webmmux",
            "matroskamux",
            "mp4mux",
            "lamemp3enc",
            "mpegaudioparse",
            "jpegdec",
        ],
    ),
    ("gstreamer1.0-plugins-ugly", &["x264enc"]),
    (
        "gstreamer1.0-plugins-bad",
        &["h264parse", "av1enc", "jpegparse"],
    ),
    ("gstreamer1.0-plugins-extra", &["vapostproc", "vah264enc"]),
];

const BUNDLED_ELEMENTS: [&str; 2] = ["gifenc", "gtk4paintablesink"];

fn package_for(element: &str) -> Option<ElementSource> {
    if BUNDLED_ELEMENTS.contains(&element) {
        return Some(ElementSource::Bundled);
    }

    PACKAGES
        .iter()
        .find(|(_, elements)| elements.contains(&element))
        .map(|(package, _)| ElementSource::Package(package))
}

fn ok(title: String, detail: Option<String>) -> CheckResult {
    CheckResult {
        title,
        status: Status::Ok,
        detail,
        failure: None,
    }
}

fn not_ok(title: String, status: Status, failure: Failure) -> CheckResult {
    CheckResult {
        title,
        status,
        detail: None,
        failure: Some(failure),
    }
}

/// Whether one of the `:`-separated names of `XDG_CURRENT_DESKTOP` is `name`.
fn desktop_is(desktop: &str, name: &str) -> bool {
    desktop
        .split(':')
        .any(|part| part.trim().eq_ignore_ascii_case(name))
}

/// Missing elements of the pipeline and of the profiles that count: the
/// non-experimental ones, plus the experimental ones when Kooha shows them or
/// when one is the active profile.
pub fn gstreamer_elements(
    find: impl Fn(&str) -> bool,
    profiles: &[ProfileElements],
    active_id: &str,
    experimental_shown: bool,
) -> CheckResult {
    let title = gettext("GStreamer Elements");

    let counted = profiles
        .iter()
        .filter(|profile| !profile.experimental || experimental_shown || profile.id == active_id)
        .collect::<Vec<_>>();

    let mut missing = Vec::<&str>::new();
    let candidates = PIPELINE_ELEMENTS
        .iter()
        .copied()
        .chain(
            counted
                .iter()
                .flat_map(|profile| profile.elements.iter().map(String::as_str)),
        )
        .chain(CAMERA_ELEMENTS);
    for element in candidates {
        if !missing.contains(&element) && !find(element) {
            missing.push(element);
        }
    }

    if missing.is_empty() {
        return ok(title, None);
    }

    let mut problem = gettext_f(
        "Missing GStreamer elements: {elements}.",
        &[("elements", &missing.join(", "))],
    );
    let mut packages = Vec::new();
    let mut bundled = Vec::new();
    for element in &missing {
        match package_for(element) {
            Some(ElementSource::Package(package)) => packages.push(package),
            Some(ElementSource::Bundled) => bundled.push(*element),
            None => {
                problem.push(' ');
                problem.push_str(&gettext_f(
                    "Unknown package for {element}.",
                    &[("element", element)],
                ));
            }
        }
    }
    // In the order of the package table.
    let packages = PACKAGES
        .iter()
        .map(|(package, _)| *package)
        .filter(|package| packages.contains(package))
        .collect::<Vec<_>>();

    let mut impacts = Vec::new();
    if PIPELINE_ELEMENTS
        .iter()
        .any(|element| missing.contains(element))
    {
        impacts.push(gettext("Recording does not work."));
    }
    let formats = counted
        .iter()
        .filter(|profile| {
            profile
                .elements
                .iter()
                .any(|element| missing.contains(&element.as_str()))
        })
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>();
    if !formats.is_empty() {
        impacts.push(gettext_f(
            "These formats cannot be used: {formats}.",
            &[("formats", &formats.join(", "))],
        ));
    }
    if CAMERA_ELEMENTS
        .iter()
        .any(|element| missing.contains(element))
    {
        impacts.push(gettext(
            "The camera overlay may not be able to show the camera.",
        ));
    }

    let failure = Failure {
        problem,
        impact: impacts.join(" "),
        fix_command: (!packages.is_empty())
            .then(|| format!("sudo apt install {}", packages.join(" "))),
        fix_note: (!bundled.is_empty()).then(|| {
            gettext_f(
                "Reinstall Kooha: it includes {elements}.",
                &[("elements", &bundled.join(", "))],
            )
        }),
    };
    // Recording works without them.
    let status = if missing
        .iter()
        .all(|element| CAMERA_ELEMENTS.contains(element))
    {
        Status::Warning
    } else {
        Status::Failed
    };
    not_ok(title, status, failure)
}

/// `result` holds the portal version and its pointer modes; `desktop` is
/// `XDG_CURRENT_DESKTOP`.
pub fn portal(result: Result<(u32, CursorMode)>, desktop: &str) -> CheckResult {
    let title = gettext("Screen Capture Portal");

    match result {
        Ok((version, cursor_modes)) => {
            let modes = [
                (CursorMode::HIDDEN, "hidden"),
                (CursorMode::EMBEDDED, "embedded"),
                (CursorMode::METADATA, "metadata"),
            ]
            .into_iter()
            .filter(|(mode, _)| cursor_modes.contains(*mode))
            .map(|(_, name)| name)
            .collect::<Vec<_>>();
            let modes = if modes.is_empty() {
                gettext("none")
            } else {
                modes.join(", ")
            };

            let detail = gettext_f(
                "Version {version} · pointer: {modes}",
                &[("version", &version.to_string()), ("modes", &modes)],
            );
            ok(title, Some(detail))
        }
        Err(err) => {
            tracing::debug!("Screen capture portal check failed: {:?}", err);

            let backend = if desktop_is(desktop, "KDE") {
                "kde"
            } else if desktop_is(desktop, "GNOME") {
                "gnome"
            } else {
                "wlr"
            };
            let failure = Failure {
                problem: gettext("The screen capture portal does not respond."),
                impact: gettext("Nothing can be recorded."),
                fix_command: Some(format!(
                    "sudo apt install xdg-desktop-portal xdg-desktop-portal-{backend}"
                )),
                fix_note: None,
            };
            not_ok(title, Status::Failed, failure)
        }
    }
}

pub fn pipewire(socket_exists: bool, version: Option<String>) -> CheckResult {
    let title = gettext("PipeWire");

    if socket_exists {
        let detail =
            version.map(|version| gettext_f("Version {version}", &[("version", &version)]));
        return ok(title, detail);
    }

    let failure = Failure {
        problem: gettext("PipeWire is not running."),
        impact: gettext("Nothing can be recorded."),
        fix_command: Some(
            "systemctl --user enable --now pipewire pipewire-pulse wireplumber".to_string(),
        ),
        fix_note: None,
    };
    not_ok(title, Status::Failed, failure)
}

/// `sink` and `source` tell whether there is a default output device and a
/// default microphone.
pub fn audio(sink: bool, source: bool) -> CheckResult {
    let title = gettext("Audio Devices");

    if sink && source {
        return ok(title, None);
    }

    let mut problems = Vec::new();
    let mut impacts = Vec::new();
    if !sink {
        problems.push(gettext("There is no default output device."));
        impacts.push(gettext("Desktop audio cannot be recorded."));
    }
    if !source {
        problems.push(gettext("There is no default microphone."));
        impacts.push(gettext("The microphone cannot be recorded."));
    }

    let failure = Failure {
        problem: problems.join(" "),
        impact: impacts.join(" "),
        fix_command: None,
        fix_note: Some(gettext(
            "Connect the device and choose it as the default in the sound settings.",
        )),
    };
    not_ok(title, Status::Failed, failure)
}

pub fn cameras(count: usize) -> CheckResult {
    let title = gettext("Cameras");

    if count > 0 {
        let n = u32::try_from(count).unwrap_or(u32::MAX);
        let detail = ngettext_f(
            "{count} camera",
            "{count} cameras",
            n,
            &[("count", &count.to_string())],
        );
        return ok(title, Some(detail));
    }

    // Only the camera overlay needs a camera.
    let failure = Failure {
        problem: gettext("No camera was found."),
        impact: gettext("The camera overlay cannot be shown."),
        fix_command: None,
        fix_note: None,
    };
    not_ok(title, Status::Warning, failure)
}

pub fn overlays(availability: Result<(), OverlayUnavailable>) -> CheckResult {
    let title = gettext("Screen Overlays");
    let impact = gettext("The camera overlay and the area border cannot be shown.");

    let (status, failure) = match availability {
        Ok(()) => return ok(title, None),
        Err(OverlayUnavailable::NotCompiled) => (
            Status::Failed,
            Failure {
                problem: gettext("Kooha was built without gtk4-layer-shell."),
                impact,
                fix_command: Some("sudo apt install libgtk4-layer-shell-dev".to_string()),
                fix_note: Some(gettext("Then rebuild Kooha.")),
            },
        ),
        Err(OverlayUnavailable::CompositorUnsupported) => (
            Status::Failed,
            Failure {
                problem: gettext(
                    "Your desktop does not support screen overlays (wlr-layer-shell).",
                ),
                impact,
                fix_command: None,
                fix_note: None,
            },
        ),
        Err(OverlayUnavailable::NoDisplay) => (
            Status::Warning,
            Failure {
                problem: gettext("Could not ask the desktop without a display."),
                impact,
                fix_command: None,
                fix_note: None,
            },
        ),
    };
    not_ok(title, status, failure)
}

/// `watcher` tells whether someone owns `org.kde.StatusNotifierWatcher`.
pub fn tray(watcher: bool, desktop: &str) -> CheckResult {
    let title = gettext("Tray Icon");

    if watcher {
        return ok(title, None);
    }

    let (fix_command, fix_note) = if desktop_is(desktop, "GNOME") {
        (
            Some("sudo apt install gnome-shell-extension-appindicator".to_string()),
            Some(gettext(
                "Then enable the AppIndicator extension and log in again.",
            )),
        )
    } else {
        (
            None,
            Some(gettext("Add a system tray to your desktop panel.")),
        )
    };
    let failure = Failure {
        problem: gettext("No system tray is available (org.kde.StatusNotifierWatcher)."),
        impact: gettext("The tray icon is not shown."),
        fix_command,
        fix_note,
    };
    not_ok(title, Status::Failed, failure)
}

pub fn recordings_folder(path: &Path, exists: bool, writable: bool) -> CheckResult {
    let title = gettext("Recordings Folder");
    let quoted = shell_quote(&path.to_string_lossy());

    let failure = if !exists {
        Failure {
            problem: gettext("The recordings folder does not exist."),
            impact: gettext("Recordings cannot be saved."),
            fix_command: Some(format!("mkdir -p -- {quoted}")),
            fix_note: None,
        }
    } else if !writable {
        Failure {
            problem: gettext("The recordings folder is not writable."),
            impact: gettext("Recordings cannot be saved."),
            fix_command: Some(format!("ls -ld -- {quoted}")),
            fix_note: Some(gettext(
                "Check the owner and the permissions of the folder, or choose another one in Preferences.",
            )),
        }
    } else {
        return ok(title, Some(path.display().to_string()));
    };

    let mut check = not_ok(title, Status::Failed, failure);
    check.detail = Some(path.display().to_string());
    check
}

/// Quotes `value` for a POSIX shell: wraps it in single quotes and writes each
/// inner `'` as `'\''`.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Whether a file can be created in `dir`: creates a temporary file there and
/// removes it. Writes nowhere else.
pub fn probe_writable(dir: &Path) -> bool {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    let probe = dir.join(format!(".kooha-write-test-{}-{nanos}", std::process::id()));

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            if let Err(err) = fs::remove_file(&probe) {
                tracing::warn!("Failed to remove `{}`: {:?}", probe.display(), err);
            }
            true
        }
        Err(err) => {
            tracing::debug!("`{}` is not writable: {:?}", dir.display(), err);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicU32, Ordering},
    };

    use anyhow::anyhow;

    use super::*;
    use crate::{doctor::Failure, profile::Profile, settings::test_utils::init_resources};

    /// A fresh, empty directory under the system temporary directory.
    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let dir = env::temp_dir().join(format!(
            "kooha-test-doctor-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn failure(check: &CheckResult) -> &Failure {
        check.failure.as_ref().expect("check has no failure")
    }

    fn profile(id: &str, name: &str, experimental: bool, elements: &[&str]) -> ProfileElements {
        ProfileElements {
            id: id.to_string(),
            name: name.to_string(),
            experimental,
            elements: elements.iter().map(|e| e.to_string()).collect(),
        }
    }

    fn profiles() -> Vec<ProfileElements> {
        vec![
            profile(
                "webm-vp8",
                "WebM",
                false,
                &["videoconvert", "vp8enc", "webmmux"],
            ),
            profile("mp4", "MP4", false, &["x264enc", "h264parse", "mp4mux"]),
            profile(
                "matroska-h264",
                "Matroska",
                false,
                &["x264enc", "matroskamux"],
            ),
            profile("webm-av1", "WebM (AV1)", true, &["av1enc", "webmmux"]),
        ]
    }

    fn missing(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |name| !names.contains(&name)
    }

    // GStreamer elements

    #[test]
    fn gstreamer_all_found() {
        let check = gstreamer_elements(|_| true, &profiles(), "webm-vp8", false);
        assert_eq!(check.title, "GStreamer Elements");
        assert_eq!(check.status, Status::Ok);
        assert!(check.failure.is_none());
    }

    #[test]
    fn gstreamer_missing_encoder_names_element_package_and_formats() {
        let check = gstreamer_elements(missing(&["x264enc"]), &profiles(), "webm-vp8", false);
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(failure.problem, "Missing GStreamer elements: x264enc.");
        assert_eq!(
            failure.impact,
            "These formats cannot be used: MP4, Matroska."
        );
        assert_eq!(
            failure.fix_command.as_deref(),
            Some("sudo apt install gstreamer1.0-plugins-ugly")
        );
    }

    /// The camera `PiP` decodes JPEG cameras with `decodebin` (fable code
    /// r1 #2); without them only the camera suffers.
    #[test]
    fn gstreamer_missing_jpeg_decoder_is_a_camera_warning() {
        let check = gstreamer_elements(
            missing(&["jpegdec", "jpegparse"]),
            &profiles(),
            "mp4",
            false,
        );
        assert_eq!(check.status, Status::Warning);
        let failure = failure(&check);
        assert_eq!(
            failure.problem,
            "Missing GStreamer elements: jpegparse, jpegdec."
        );
        assert_eq!(
            failure.impact,
            "The camera overlay may not be able to show the camera."
        );
        assert_eq!(
            failure.fix_command.as_deref(),
            Some("sudo apt install gstreamer1.0-plugins-good gstreamer1.0-plugins-bad")
        );
    }

    #[test]
    fn gstreamer_missing_decodebin_with_an_encoder_fails() {
        let check = gstreamer_elements(
            missing(&["decodebin", "x264enc"]),
            &profiles(),
            "mp4",
            false,
        );
        assert_eq!(check.status, Status::Failed);
        assert_eq!(
            failure(&check).impact,
            "These formats cannot be used: MP4, Matroska. The camera overlay may not be able to show \
             the camera."
        );
    }

    #[test]
    fn gstreamer_missing_pipeline_elements() {
        let check = gstreamer_elements(
            missing(&["pipewiresrc", "vp8enc", "videocrop"]),
            &profiles(),
            "mp4",
            false,
        );
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(
            failure.problem,
            "Missing GStreamer elements: pipewiresrc, videocrop, vp8enc."
        );
        assert_eq!(
            failure.impact,
            "Recording does not work. These formats cannot be used: WebM."
        );
        assert_eq!(
            failure.fix_command.as_deref(),
            Some("sudo apt install gstreamer1.0-pipewire gstreamer1.0-plugins-good")
        );
    }

    #[test]
    fn gstreamer_experimental_profiles_do_not_count() {
        let check = gstreamer_elements(missing(&["av1enc"]), &profiles(), "webm-vp8", false);
        assert_eq!(check.status, Status::Ok);
    }

    #[test]
    fn gstreamer_active_experimental_profile_counts() {
        let check = gstreamer_elements(missing(&["av1enc"]), &profiles(), "webm-av1", false);
        assert_eq!(check.status, Status::Failed);
        assert_eq!(
            failure(&check).fix_command.as_deref(),
            Some("sudo apt install gstreamer1.0-plugins-bad")
        );
    }

    #[test]
    fn gstreamer_shown_experimental_profiles_count() {
        let check = gstreamer_elements(missing(&["av1enc"]), &profiles(), "webm-vp8", true);
        assert_eq!(check.status, Status::Failed);
    }

    #[test]
    fn gstreamer_unknown_package() {
        let mut profiles = profiles();
        profiles[0].elements.push("fancyenc".to_string());

        let check = gstreamer_elements(missing(&["fancyenc"]), &profiles, "mp4", false);
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(
            failure.problem,
            "Missing GStreamer elements: fancyenc. Unknown package for fancyenc."
        );
        assert_eq!(failure.fix_command, None);
    }

    #[test]
    fn gstreamer_bundled_element() {
        let mut profiles = profiles();
        profiles[0].elements.push("gifenc".to_string());

        let check = gstreamer_elements(missing(&["gifenc"]), &profiles, "mp4", false);
        let failure = failure(&check);
        assert_eq!(failure.fix_command, None);
        assert_eq!(
            failure.fix_note.as_deref(),
            Some("Reinstall Kooha: it includes gifenc.")
        );
    }

    #[test]
    fn every_element_has_a_package() {
        init_resources();

        let profile_elements = Profile::all()
            .unwrap()
            .iter()
            .flat_map(|profile| profile.required_elements());
        let pipeline_elements = PIPELINE_ELEMENTS
            .iter()
            .chain(&CAMERA_ELEMENTS)
            .map(|e| e.to_string());

        for element in profile_elements.chain(pipeline_elements) {
            assert!(
                package_for(&element).is_some(),
                "`{element}` is not in the package table"
            );
        }
    }

    // Portal

    #[test]
    fn portal_ok_reports_version_and_pointer_modes() {
        let check = portal(Ok((5, CursorMode::EMBEDDED | CursorMode::METADATA)), "KDE");
        assert_eq!(check.title, "Screen Capture Portal");
        assert_eq!(check.status, Status::Ok);
        assert_eq!(
            check.detail.as_deref(),
            Some("Version 5 · pointer: embedded, metadata")
        );
        assert!(check.failure.is_none());
    }

    #[test]
    fn portal_ok_without_pointer_modes() {
        let check = portal(Ok((4, CursorMode::empty())), "KDE");
        assert_eq!(check.detail.as_deref(), Some("Version 4 · pointer: none"));
    }

    #[test]
    fn portal_failed_by_desktop() {
        for (desktop, backend) in [
            ("KDE", "kde"),
            ("ubuntu:GNOME", "gnome"),
            ("sway", "wlr"),
            ("", "wlr"),
        ] {
            let check = portal(Err(anyhow!("no portal")), desktop);
            assert_eq!(check.status, Status::Failed);
            let failure = failure(&check);
            assert_eq!(
                failure.problem,
                "The screen capture portal does not respond."
            );
            assert_eq!(failure.impact, "Nothing can be recorded.");
            assert_eq!(
                failure.fix_command,
                Some(format!(
                    "sudo apt install xdg-desktop-portal xdg-desktop-portal-{backend}"
                ))
            );
        }
    }

    // PipeWire

    #[test]
    fn pipewire_ok_reports_version() {
        let check = pipewire(true, Some("1.0.5".to_string()));
        assert_eq!(check.title, "PipeWire");
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.detail.as_deref(), Some("Version 1.0.5"));

        assert_eq!(pipewire(true, None).status, Status::Ok);
    }

    #[test]
    fn pipewire_without_socket() {
        let check = pipewire(false, None);
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(failure.problem, "PipeWire is not running.");
        assert_eq!(failure.impact, "Nothing can be recorded.");
        assert_eq!(
            failure.fix_command.as_deref(),
            Some("systemctl --user enable --now pipewire pipewire-pulse wireplumber")
        );
    }

    // Audio

    #[test]
    fn audio_ok() {
        let check = audio(true, true);
        assert_eq!(check.title, "Audio Devices");
        assert_eq!(check.status, Status::Ok);
        assert!(check.failure.is_none());
    }

    #[test]
    fn audio_without_output() {
        let check = audio(false, true);
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(failure.problem, "There is no default output device.");
        assert_eq!(failure.impact, "Desktop audio cannot be recorded.");
        assert_eq!(failure.fix_command, None);
        assert!(failure.fix_note.is_some());
    }

    #[test]
    fn audio_without_microphone() {
        let failure = audio(true, false).failure.unwrap();
        assert_eq!(failure.problem, "There is no default microphone.");
        assert_eq!(failure.impact, "The microphone cannot be recorded.");
    }

    #[test]
    fn audio_without_both() {
        let failure = audio(false, false).failure.unwrap();
        assert_eq!(
            failure.problem,
            "There is no default output device. There is no default microphone."
        );
        assert_eq!(
            failure.impact,
            "Desktop audio cannot be recorded. The microphone cannot be recorded."
        );
    }

    // Cameras

    #[test]
    fn cameras_none_is_a_warning() {
        let check = cameras(0);
        assert_eq!(check.title, "Cameras");
        assert_eq!(check.status, Status::Warning);
        let failure = failure(&check);
        assert_eq!(failure.problem, "No camera was found.");
        assert_eq!(failure.impact, "The camera overlay cannot be shown.");
        assert_eq!(failure.fix_command, None);
    }

    #[test]
    fn cameras_found() {
        let check = cameras(2);
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.detail.as_deref(), Some("2 cameras"));
        assert_eq!(cameras(1).detail.as_deref(), Some("1 camera"));
    }

    // Overlays

    #[test]
    fn overlays_ok() {
        let check = overlays(Ok(()));
        assert_eq!(check.title, "Screen Overlays");
        assert_eq!(check.status, Status::Ok);
    }

    #[test]
    fn overlays_not_compiled() {
        let check = overlays(Err(OverlayUnavailable::NotCompiled));
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(failure.problem, "Kooha was built without gtk4-layer-shell.");
        assert_eq!(
            failure.impact,
            "The camera overlay and the area border cannot be shown."
        );
        assert_eq!(
            failure.fix_command.as_deref(),
            Some("sudo apt install libgtk4-layer-shell-dev")
        );
        assert_eq!(failure.fix_note.as_deref(), Some("Then rebuild Kooha."));
    }

    #[test]
    fn overlays_compositor_unsupported() {
        let check = overlays(Err(OverlayUnavailable::CompositorUnsupported));
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(
            failure.problem,
            "Your desktop does not support screen overlays (wlr-layer-shell)."
        );
        assert_eq!(failure.fix_command, None);
    }

    #[test]
    fn overlays_no_display_is_a_warning() {
        let check = overlays(Err(OverlayUnavailable::NoDisplay));
        assert_eq!(check.status, Status::Warning);
        let failure = failure(&check);
        assert_eq!(
            failure.problem,
            "Could not ask the desktop without a display."
        );
        assert_eq!(failure.fix_command, None);
    }

    // Tray

    #[test]
    fn tray_ok() {
        let check = tray(true, "KDE");
        assert_eq!(check.title, "Tray Icon");
        assert_eq!(check.status, Status::Ok);
    }

    #[test]
    fn tray_without_watcher_on_gnome() {
        let check = tray(false, "ubuntu:GNOME");
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(
            failure.problem,
            "No system tray is available (org.kde.StatusNotifierWatcher)."
        );
        assert_eq!(failure.impact, "The tray icon is not shown.");
        assert_eq!(
            failure.fix_command.as_deref(),
            Some("sudo apt install gnome-shell-extension-appindicator")
        );
    }

    #[test]
    fn tray_without_watcher_elsewhere() {
        let check = tray(false, "sway");
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(failure.fix_command, None);
        assert!(failure.fix_note.is_some());
    }

    // Recordings folder

    #[test]
    fn recordings_folder_ok() {
        let check = recordings_folder(Path::new("/home/me/Videos/Kooha"), true, true);
        assert_eq!(check.title, "Recordings Folder");
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.detail.as_deref(), Some("/home/me/Videos/Kooha"));
    }

    #[test]
    fn recordings_folder_missing() {
        let check = recordings_folder(Path::new("/home/me/My 'Videos'"), false, false);
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(failure.problem, "The recordings folder does not exist.");
        assert_eq!(failure.impact, "Recordings cannot be saved.");
        assert_eq!(
            failure.fix_command.as_deref(),
            Some(r"mkdir -p -- '/home/me/My '\''Videos'\'''")
        );
    }

    #[test]
    fn recordings_folder_not_writable() {
        let check = recordings_folder(Path::new("/srv/rec"), true, false);
        assert_eq!(check.status, Status::Failed);
        let failure = failure(&check);
        assert_eq!(failure.problem, "The recordings folder is not writable.");
        assert_eq!(failure.fix_command.as_deref(), Some("ls -ld -- '/srv/rec'"));
        assert!(failure.fix_note.is_some());
    }

    #[test]
    fn probe_writable_leaves_nothing_behind() {
        let dir = temp_dir("writable");

        assert!(probe_writable(&dir));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn probe_read_only_folder() {
        let dir = temp_dir("read-only");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();

        assert!(!probe_writable(&dir));
        assert!(!probe_writable(&dir.join("missing")));

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    // shell_quote

    /// Runs `printf '%s\0' <quoted>` in `sh` and returns its output.
    fn shell_round_trip(value: &str) -> Vec<u8> {
        let script = format!("printf '%s\\0' {}", shell_quote(value));
        let output = Command::new("sh").arg("-c").arg(script).output().unwrap();
        assert!(output.status.success(), "sh failed for `{value}`");
        output.stdout
    }

    #[test]
    fn shell_quote_is_one_literal_argument() {
        let dir = temp_dir("quote");
        let marker1 = dir.join("marker1");
        let marker2 = dir.join("marker2");

        let values = [
            "/home/me/My Videos".to_string(),
            "/home/me/it's here".to_string(),
            "/home/me/say \"hi\"".to_string(),
            "-rf".to_string(),
            "''".to_string(),
            String::new(),
            format!("/tmp/$(touch {})", marker1.display()),
            format!("/tmp/`touch {}`", marker2.display()),
            format!("'; touch {}; '", marker1.display()),
        ];

        for value in &values {
            let mut expected = value.as_bytes().to_vec();
            expected.push(0);
            assert_eq!(shell_round_trip(value), expected, "for `{value}`");
        }

        assert!(!marker1.exists());
        assert!(!marker2.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shell_quote_format() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
