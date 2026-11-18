use std::path::Path;

use futures_channel::mpsc::UnboundedSender;
use gettextrs::gettext;
use ksni::{
    MenuItem,
    blocking::{Handle, TrayMethods},
    menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu},
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
            let reason = err.to_string();
            tracing::warn!(
                "Failed to send tray command {:?}: {}",
                err.into_inner().command,
                reason
            );
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
        menu_items(model::menu_entries(&self.model))
    }
}

/// The menu items of `entries`, with each run of radios as one group.
fn menu_items(entries: Vec<TrayEntry>) -> Vec<MenuItem<KoohaTray>> {
    let mut items = Vec::new();
    let mut radios = Vec::new();

    for entry in entries {
        if matches!(entry, TrayEntry::Radio { .. }) {
            radios.push(entry);
            continue;
        }
        if !radios.is_empty() {
            items.push(radio_group(std::mem::take(&mut radios)));
        }
        items.push(menu_item(entry));
    }
    if !radios.is_empty() {
        items.push(radio_group(radios));
    }

    items
}

/// One group of `radios`, which are all `TrayEntry::Radio`.
fn radio_group(radios: Vec<TrayEntry>) -> MenuItem<KoohaTray> {
    let mut options = Vec::new();
    let mut commands = Vec::new();
    // `selected` is required: with none selected, an index no radio has.
    let mut selected_index = usize::MAX;

    for (index, radio) in radios.into_iter().enumerate() {
        let TrayEntry::Radio {
            label,
            command,
            enabled,
            selected,
        } = radio
        else {
            unreachable!("only radios are grouped");
        };
        if selected {
            selected_index = index;
        }
        options.push(RadioItem {
            label,
            enabled,
            ..Default::default()
        });
        commands.push(command);
    }

    RadioGroup {
        selected: selected_index,
        select: Box::new(move |tray: &mut KoohaTray, index| {
            if let Some(command) = commands.get(index) {
                tray.send(command.clone());
            }
        }),
        options,
    }
    .into()
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
            activate: Box::new(move |tray: &mut KoohaTray| tray.send(command.clone())),
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
            activate: Box::new(move |tray: &mut KoohaTray| tray.send(command.clone())),
            ..Default::default()
        }
        .into(),
        TrayEntry::SubMenu { label, children } => SubMenu {
            label,
            submenu: menu_items(children),
            ..Default::default()
        }
        .into(),
        radio @ TrayEntry::Radio { .. } => radio_group(vec![radio]),
        TrayEntry::Separator => MenuItem::Separator,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use futures_channel::mpsc;
    use ksni::{MenuItem, Tray};

    use super::*;
    use crate::{
        areas::Selected,
        tray::model::{self, TrayArea, TrayRecordingState},
    };

    fn idle_model() -> TrayModel {
        TrayModel {
            state: TrayRecordingState::Idle,
            duration_text: "00:00".to_string(),
            warnings: Vec::new(),
            record_desktop_audio: true,
            record_microphone: true,
            show_pointer: true,
            audio_supported: true,
            camera_enabled: false,
            camera_available: true,
            areas_available: false,
            areas: Vec::new(),
            show_area_border: true,
        }
    }

    fn area(name: &str, chosen: bool) -> TrayArea {
        TrayArea {
            label: name.to_string(),
            selected: Selected::Named(name.to_string()),
            enabled: true,
            chosen,
        }
    }

    fn tray_with_areas(areas: Vec<TrayArea>) -> (KoohaTray, mpsc::UnboundedReceiver<TrayRequest>) {
        let (sender, receiver) = mpsc::unbounded();
        let mut model = idle_model();
        model.areas_available = true;
        model.areas = areas;
        let tray = KoohaTray {
            model,
            sender,
            activation_token: fake_token,
        };
        (tray, receiver)
    }

    fn area_submenu(items: &[MenuItem<KoohaTray>]) -> &[MenuItem<KoohaTray>] {
        items
            .iter()
            .find_map(|item| match item {
                MenuItem::SubMenu(submenu) if submenu.label == "Area" => Some(&submenu.submenu[..]),
                _ => None,
            })
            .expect("menu has the Area submenu")
    }

    fn command(receiver: &mut mpsc::UnboundedReceiver<TrayRequest>) -> TrayCommand {
        receiver.try_recv().expect("a command was sent").command
    }

    /// The radios become one group, then the separator and Mark New Area.
    #[test]
    fn area_submenu_maps_radios_to_one_group() {
        let (mut tray, mut receiver) =
            tray_with_areas(vec![area("Demo", false), area("Talk", true)]);

        let items = tray.menu();
        let submenu = area_submenu(&items);

        assert_eq!(submenu.len(), 3);
        let MenuItem::RadioGroup(group) = &submenu[0] else {
            panic!("expected a radio group, got {:?}", submenu[0]);
        };
        assert_eq!(group.options.len(), 2);
        assert_eq!(group.options[1].label, "Talk");
        assert_eq!(group.selected, 1);
        assert!(matches!(submenu[1], MenuItem::Separator));
        assert!(matches!(&submenu[2], MenuItem::Standard(item) if item.label == "Mark New Area…"));

        (group.select)(&mut tray, 0);
        assert_eq!(
            command(&mut receiver),
            TrayCommand::SelectArea(Selected::Named("Demo".to_string()))
        );
    }

    /// `RadioGroup::selected` is required: with none chosen, an index no
    /// radio has.
    #[test]
    fn no_chosen_area_selects_no_radio() {
        let (tray, _receiver) = tray_with_areas(vec![area("Demo", false), area("Talk", false)]);

        let items = tray.menu();
        let MenuItem::RadioGroup(group) = &area_submenu(&items)[0] else {
            panic!("expected a radio group");
        };
        assert_eq!(group.selected, usize::MAX);
    }

    /// ksni calls the same callback for each click, so each one clones the
    /// command.
    #[test]
    fn two_clicks_send_two_commands() {
        let (mut tray, mut receiver) = tray_with_areas(vec![area("Demo", false)]);

        let items = tray.menu();
        let submenu = area_submenu(&items);
        let MenuItem::Standard(mark) = &submenu[2] else {
            panic!("expected Mark New Area");
        };
        (mark.activate)(&mut tray);
        (mark.activate)(&mut tray);
        let MenuItem::RadioGroup(group) = &submenu[0] else {
            panic!("expected a radio group");
        };
        (group.select)(&mut tray, 0);
        (group.select)(&mut tray, 0);

        assert_eq!(command(&mut receiver), TrayCommand::MarkArea);
        assert_eq!(command(&mut receiver), TrayCommand::MarkArea);
        let demo = TrayCommand::SelectArea(Selected::Named("Demo".to_string()));
        assert_eq!(command(&mut receiver), demo);
        assert_eq!(command(&mut receiver), demo);
        assert!(receiver.try_recv().is_err());
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
