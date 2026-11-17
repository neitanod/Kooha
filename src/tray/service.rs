use std::path::Path;

use futures_channel::mpsc::UnboundedSender;
use gettextrs::gettext;
use ksni::{
    MenuItem,
    blocking::{Handle, TrayMethods},
    menu::{CheckmarkItem, StandardItem},
};

use super::model::{self, TrayCommand, TrayEntry, TrayModel};
use crate::config::{APP_ID, PKGDATADIR};

/// A tray click: its command, with the XDG activation token the host gave
/// for it, which lets the window come to the front on Wayland.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayRequest {
    pub command: TrayCommand,
    pub activation_token: Option<String>,
}

/// The `StatusNotifierItem` of the app. Without a `StatusNotifierWatcher` on
/// the session bus there is no icon until one appears; if the service cannot
/// start at all, every method does nothing.
pub struct TrayService {
    handle: Option<Handle<KoohaTray>>,
}

impl TrayService {
    /// Registers the tray icon. Menu clicks arrive through `sender`, sent
    /// from the tray thread.
    pub fn spawn(sender: UnboundedSender<TrayRequest>, model: TrayModel) -> Self {
        let tray = KoohaTray {
            model,
            sender,
            activation_token: ksni::current_xdg_activation_token,
        };

        // Without a watcher or a host yet (a panel that starts after Kooha,
        // or restarts), the icon registers once they show up instead of
        // never (fable code r1 #5).
        // In Flatpak the sandbox may not own `org.kde.StatusNotifierItem-*`,
        // so the item registers with its unique bus name, as ksni advises
        // (codex code r1 #2).
        match tray
            .assume_sni_available(true)
            .disable_dbus_name(is_sandboxed_at(Path::new("/")))
            .spawn()
        {
            Ok(handle) => Self {
                handle: Some(handle),
            },
            Err(err) => {
                tracing::warn!("Failed to show the tray icon: {}", err);
                Self { handle: None }
            }
        }
    }

    pub fn update(&self, model: TrayModel) {
        if let Some(handle) = &self.handle {
            handle.update(|tray| tray.model = model);
        }
    }
}

impl std::fmt::Debug for TrayService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrayService")
            .field("is_running", &self.handle.is_some())
            .finish()
    }
}

/// Whether `root` is the root of a Flatpak sandbox.
fn is_sandboxed_at(root: &Path) -> bool {
    root.join(".flatpak-info").exists()
}

struct KoohaTray {
    model: TrayModel,
    sender: UnboundedSender<TrayRequest>,
    /// `ksni::current_xdg_activation_token`, replaced in tests.
    activation_token: fn() -> Option<String>,
}

impl KoohaTray {
    fn send(&self, command: TrayCommand) {
        // Runs on the tray thread, so it must not touch GTK. The token only
        // exists while ksni handles the click, so it is read here. It may
        // also arrive a little after the click; waiting for it here would
        // block the thread that receives it, so a late one is not used.
        let request = TrayRequest {
            command,
            activation_token: (self.activation_token)(),
        };
        if let Err(err) = self.sender.unbounded_send(request) {
            tracing::warn!("Failed to send tray command {:?}: {}", command, err);
        }
    }
}

impl ksni::Tray for KoohaTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        gettext("Kooha")
    }

    fn icon_name(&self) -> String {
        model::icon_name(self.model.state).into()
    }

    fn icon_theme_path(&self) -> String {
        // Lets the host find the app icon also when it is installed in a
        // custom prefix.
        Path::new(PKGDATADIR)
            .parent()
            .map(|datadir| datadir.join("icons").display().to_string())
            .unwrap_or_default()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let (title, description) = model::tooltip(&self.model);
        ksni::ToolTip {
            title,
            description,
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayCommand::ShowWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        model::menu_entries(&self.model)
            .into_iter()
            .map(menu_item)
            .collect()
    }
}

fn menu_item(entry: TrayEntry) -> MenuItem<KoohaTray> {
    match entry {
        TrayEntry::Label(label) => StandardItem {
            label,
            enabled: false,
            ..Default::default()
        }
        .into(),
        TrayEntry::Button {
            label,
            command,
            enabled,
        } => StandardItem {
            label,
            enabled,
            activate: Box::new(move |tray: &mut KoohaTray| tray.send(command)),
            ..Default::default()
        }
        .into(),
        TrayEntry::Check {
            label,
            checked,
            command,
            enabled,
        } => CheckmarkItem {
            label,
            checked,
            enabled,
            activate: Box::new(move |tray: &mut KoohaTray| tray.send(command)),
            ..Default::default()
        }
        .into(),
        TrayEntry::Separator => MenuItem::Separator,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use futures_channel::mpsc;
    use ksni::{MenuItem, Tray};

    use super::*;
    use crate::tray::model::{self, TrayRecordingState};

    fn idle_model() -> TrayModel {
        TrayModel {
            state: TrayRecordingState::Idle,
            duration_text: "00:00".to_string(),
            warnings: Vec::new(),
            record_desktop_audio: true,
            record_microphone: true,
            show_pointer: true,
            audio_supported: true,
        }
    }

    #[test]
    fn sandboxed_when_flatpak_info_exists() {
        let root = std::env::temp_dir().join(format!("kooha-test-sandbox-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        assert!(!is_sandboxed_at(&root));
        std::fs::write(root.join(".flatpak-info"), "[Application]\n").unwrap();
        assert!(is_sandboxed_at(&root));

        std::fs::remove_dir_all(&root).unwrap();
    }

    fn fake_token() -> Option<String> {
        Some("kwin-token-42".to_string())
    }

    #[test]
    fn tray_menu_maps_entries() {
        let (sender, _receiver) = mpsc::unbounded();
        let tray = KoohaTray {
            model: idle_model(),
            sender,
            activation_token: fake_token,
        };

        let items = tray.menu();

        assert_eq!(items.len(), model::menu_entries(&tray.model).len());
        match &items[0] {
            MenuItem::Standard(item) => {
                assert_eq!(item.label, "Ready");
                assert!(!item.enabled);
            }
            other => panic!("expected a standard item, got {other:?}"),
        }
    }

    /// Each command carries the activation token of its click, so that the
    /// window can come to the front on Wayland (fable code r1 #4).
    #[test]
    fn menu_item_sends_its_command_with_token() {
        let (sender, mut receiver) = mpsc::unbounded();
        let mut tray = KoohaTray {
            model: idle_model(),
            sender,
            activation_token: fake_token,
        };

        let items = tray.menu();
        let MenuItem::Checkmark(microphone) = items
            .iter()
            .find(|item| matches!(item, MenuItem::Checkmark(c) if c.label == "Record Microphone"))
            .unwrap()
        else {
            unreachable!()
        };
        (microphone.activate)(&mut tray);
        tray.activate(0, 0);

        let quit = items
            .iter()
            .find_map(|item| match item {
                MenuItem::Standard(item) if item.label == "Quit" => Some(item),
                _ => None,
            })
            .unwrap();
        (quit.activate)(&mut tray);

        for command in [
            TrayCommand::ToggleMicrophone,
            TrayCommand::ShowWindow,
            TrayCommand::Quit,
        ] {
            assert_eq!(
                receiver.try_recv().unwrap(),
                TrayRequest {
                    command,
                    activation_token: Some("kwin-token-42".to_string()),
                }
            );
        }
    }

    /// Needs a session bus of its own, without a `StatusNotifierWatcher`:
    ///
    /// `dbus-run-session -- cargo test --target-dir _build/src -- --ignored tray::service::tests::spawn_without_watcher_does_not_fail`
    #[test]
    #[ignore = "needs a private session bus, see the doc comment"]
    fn spawn_without_watcher_does_not_fail() {
        let started = Instant::now();
        let (sender, _receiver) = mpsc::unbounded();

        let service = TrayService::spawn(sender, idle_model());
        service.update(idle_model());

        // Kept, to register the icon once a watcher shows up (fable code r1
        // #5).
        assert!(service.handle.is_some());
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
    }
}
