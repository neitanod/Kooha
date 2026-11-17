//! Requirements report: checks what Kooha needs from the system and explains
//! how to fix what is missing.

pub mod checks;
pub mod dialog;

use std::{env, path::PathBuf, time::Duration};

use anyhow::Result;
use gettextrs::gettext;
use gst::prelude::*;
use gtk::{gio, glib, prelude::*};

use self::checks::ProfileElements;
use crate::{
    camera_pip::devices,
    device::{self, DeviceClass},
    experimental::Feature,
    overlay::OverlayUnavailable,
    profile::Profile,
    screencast_portal::{self, CursorMode},
    settings::Settings,
};

/// How long to wait for an answer on the session bus.
const DBUS_TIMEOUT: Duration = Duration::from_secs(5);

const STATUS_NOTIFIER_WATCHER: &str = "org.kde.StatusNotifierWatcher";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warning,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub problem: String,
    pub impact: String,
    pub fix_command: Option<String>,
    /// For example "Then rebuild Kooha."
    pub fix_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub title: String,
    pub status: Status,
    /// Shown under the title, for example "Version 5".
    pub detail: Option<String>,
    /// Some only when `status` is not `Ok`.
    pub failure: Option<Failure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report(pub Vec<CheckResult>);

impl Report {
    /// Whether any check failed. Warnings do not count.
    pub fn has_failures(&self) -> bool {
        self.0.iter().any(|check| check.status == Status::Failed)
    }

    /// The report as plain text, for "Copy Report" and `kooha --check`.
    pub fn to_text(&self) -> String {
        let mut text = String::new();

        for check in &self.0 {
            let mark = match check.status {
                Status::Ok => "[OK]",
                Status::Warning => "[WARN]",
                Status::Failed => "[FAIL]",
            };
            text.push_str(&format!("{mark} {}", check.title));
            if let Some(detail) = &check.detail {
                text.push_str(&format!(": {detail}"));
            }
            text.push('\n');

            let Some(failure) = check
                .failure
                .as_ref()
                .filter(|_| check.status != Status::Ok)
            else {
                continue;
            };

            text.push_str(&format!(
                "    {}: {}\n",
                gettext("Problem"),
                failure.problem
            ));
            text.push_str(&format!(
                "    {}: {}\n",
                gettext("What stops working"),
                failure.impact
            ));
            match (&failure.fix_command, &failure.fix_note) {
                (Some(command), note) => {
                    text.push_str(&format!("    {}: {command}\n", gettext("How to fix it")));
                    if let Some(note) = note {
                        text.push_str(&format!("    {note}\n"));
                    }
                }
                (None, Some(note)) => {
                    text.push_str(&format!("    {}: {note}\n", gettext("How to fix it")));
                }
                (None, None) => {}
            }
        }

        text
    }
}

/// Runs the eight checks in the order of the spec. They only read the system,
/// except the recordings folder check, which creates and removes a temporary
/// file in that folder.
///
/// `overlays` is `overlay::availability()`, decided by the caller because it
/// depends on whether GTK has a display.
pub async fn run(settings: &Settings, overlays: Result<(), OverlayUnavailable>) -> Report {
    let desktop = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();

    let gstreamer = checks::gstreamer_elements(
        |name| gst::ElementFactory::find(name).is_some(),
        &profile_elements(),
        &settings.string("profile-id"),
        Feature::ExperimentalFormats.is_enabled(),
    );
    let portal = checks::portal(query_portal().await, &desktop);
    let pipewire = checks::pipewire(pipewire_socket().exists(), pipewire_version());
    let audio = checks::audio(
        device::find_default(DeviceClass::Sink).is_ok(),
        device::find_default(DeviceClass::Source).is_ok(),
    );
    let cameras = checks::cameras(devices::list().len());
    let overlays = checks::overlays(overlays);
    let tray = checks::tray(has_status_notifier_watcher().await, &desktop);

    let folder = settings.saving_location_unchecked();
    let folder_exists = folder.is_dir();
    let folder_writable = folder_exists && checks::probe_writable(&folder);
    let recordings_folder = checks::recordings_folder(&folder, folder_exists, folder_writable);

    Report(vec![
        gstreamer,
        portal,
        pipewire,
        audio,
        cameras,
        overlays,
        tray,
        recordings_folder,
    ])
}

fn profile_elements() -> Vec<ProfileElements> {
    let profiles = Profile::all().unwrap_or_else(|err| {
        tracing::warn!("Failed to load the profiles: {:?}", err);
        &[]
    });

    profiles
        .iter()
        .map(|profile| ProfileElements {
            id: profile.id().to_string(),
            name: profile.name().to_string(),
            experimental: profile.is_experimental(),
            elements: profile.required_elements(),
        })
        .collect()
}

/// Reads the version and the pointer modes of the screen cast portal, without
/// creating a session.
async fn query_portal() -> Result<(u32, CursorMode)> {
    let proxy = glib::future_with_timeout(DBUS_TIMEOUT, screencast_portal::Proxy::new()).await??;
    Ok((proxy.version()?, proxy.available_cursor_modes()?))
}

fn pipewire_socket() -> PathBuf {
    env::var_os("PIPEWIRE_RUNTIME_DIR")
        .map_or_else(glib::user_runtime_dir, PathBuf::from)
        .join("pipewire-0")
}

/// The version of the plugin that holds `pipewiresrc`, as in the About dialog.
fn pipewire_version() -> Option<String> {
    gst::Registry::get()
        .find_feature("pipewiresrc", gst::ElementFactory::static_type())
        .and_then(|feature| feature.plugin())
        .map(|plugin| plugin.version().to_string())
}

async fn has_status_notifier_watcher() -> bool {
    let query = async {
        let connection = gio::bus_get_future(gio::BusType::Session).await?;
        let reply = connection
            .call_future(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                Some(&(STATUS_NOTIFIER_WATCHER,).to_variant()),
                Some(glib::VariantTy::new("(b)").unwrap()),
                gio::DBusCallFlags::NONE,
                i32::try_from(DBUS_TIMEOUT.as_millis()).unwrap_or(i32::MAX),
            )
            .await?;
        Ok::<_, glib::Error>(reply.get::<(bool,)>().is_some_and(|(owned,)| owned))
    };

    match glib::future_with_timeout(DBUS_TIMEOUT, query).await {
        Ok(Ok(owned)) => owned,
        Ok(Err(err)) => {
            tracing::debug!("Failed to ask for `{}`: {:?}", STATUS_NOTIFIER_WATCHER, err);
            false
        }
        Err(err) => {
            tracing::debug!(
                "Timed out asking for `{}`: {:?}",
                STATUS_NOTIFIER_WATCHER,
                err
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(title: &str, status: Status) -> CheckResult {
        let failure = (status != Status::Ok).then(|| Failure {
            problem: format!("{title} problem"),
            impact: format!("{title} impact"),
            fix_command: Some(format!("fix-{title}")),
            fix_note: Some(format!("{title} note")),
        });

        CheckResult {
            title: title.to_string(),
            status,
            detail: Some(format!("{title} detail")),
            failure,
        }
    }

    #[test]
    fn to_text_marks_each_check() {
        let report = Report(vec![
            check("A", Status::Ok),
            check("B", Status::Warning),
            check("C", Status::Failed),
        ]);

        let text = report.to_text();
        let titles = text
            .lines()
            .filter(|line| line.starts_with('['))
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            [
                "[OK] A: A detail",
                "[WARN] B: B detail",
                "[FAIL] C: C detail"
            ]
        );
    }

    #[test]
    fn to_text_explains_only_warnings_and_failures() {
        let report = Report(vec![
            check("A", Status::Ok),
            check("B", Status::Warning),
            check("C", Status::Failed),
        ]);

        let text = report.to_text();
        assert_eq!(
            text,
            "[OK] A: A detail\n\
             [WARN] B: B detail\n\
             \x20   Problem: B problem\n\
             \x20   What stops working: B impact\n\
             \x20   How to fix it: fix-B\n\
             \x20   B note\n\
             [FAIL] C: C detail\n\
             \x20   Problem: C problem\n\
             \x20   What stops working: C impact\n\
             \x20   How to fix it: fix-C\n\
             \x20   C note\n"
        );
        assert!(!text.contains("A problem"));
    }

    #[test]
    fn to_text_without_detail_or_command() {
        let mut failed = check("C", Status::Failed);
        failed.detail = None;
        let failure = failed.failure.as_mut().unwrap();
        failure.fix_command = None;
        failure.fix_note = None;
        let mut ok = check("A", Status::Ok);
        ok.detail = None;

        assert_eq!(
            Report(vec![ok, failed]).to_text(),
            "[OK] A\n\
             [FAIL] C\n\
             \x20   Problem: C problem\n\
             \x20   What stops working: C impact\n"
        );
    }

    #[test]
    fn note_without_command() {
        let mut failed = check("C", Status::Failed);
        failed.failure.as_mut().unwrap().fix_command = None;

        assert!(
            Report(vec![failed])
                .to_text()
                .ends_with("    What stops working: C impact\n    How to fix it: C note\n")
        );
    }

    #[test]
    fn has_failures_ignores_warnings() {
        assert!(!Report(vec![check("A", Status::Ok)]).has_failures());
        assert!(!Report(vec![check("A", Status::Ok), check("B", Status::Warning)]).has_failures());
        assert!(
            Report(vec![
                check("B", Status::Warning),
                check("C", Status::Failed)
            ])
            .has_failures()
        );
    }
}
