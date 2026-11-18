//! After marking: «1280×720 on DP-3», with an optional name to save it as.

use adw::prelude::*;
use gettextrs::gettext;

use crate::{areas::MarkedArea, i18n::gettext_f};

/// What to do with a marked area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveChoice {
    /// Keep it as the last area, out of the list.
    Last,
    /// Save it in the list with this name, replacing any area with it.
    Named(String),
    /// Mark it again.
    MarkAgain,
}

/// Where a name typed in the dialog leads.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    Done(SaveChoice),
    /// The name is taken: ask whether to replace that area.
    AskReplace(String),
    /// Back to the first dialog, with this name to change.
    EditName(String),
}

/// Asks whether to save `area` with a name. `name_exists` tells whether the
/// saved list already has a name; when the user chose to replace it, the
/// answer is `Named` and the caller saves with `replace = true`.
// The window calls it after marking (chunk 5).
#[allow(dead_code)]
pub async fn ask(
    parent: &impl IsA<gtk::Widget>,
    area: &MarkedArea,
    name_exists: impl Fn(&str) -> bool,
) -> SaveChoice {
    let mut name = String::new();
    loop {
        let (dialog, entry_row) = save_dialog(area, &name);
        if dialog.choose_future(Some(parent)).await == "remark" {
            return SaveChoice::MarkAgain;
        }
        let text = entry_row.text();

        let decision = match choice_for(&text, name_exists(text.trim()), None) {
            Decision::AskReplace(taken) => {
                let response = replace_dialog(&taken, area)
                    .choose_future(Some(parent))
                    .await;
                choice_for(&taken, true, Some(response == "replace"))
            }
            decision => decision,
        };
        match decision {
            Decision::Done(choice) => return choice,
            Decision::AskReplace(other) | Decision::EditName(other) => name = other,
        }
    }
}

/// Spaces around the name do not count; no name keeps the area as the
/// last one.
fn choice_for(name: &str, exists: bool, replace_answer: Option<bool>) -> Decision {
    let name = name.trim();
    if name.is_empty() {
        return Decision::Done(SaveChoice::Last);
    }
    match (exists, replace_answer) {
        (false, _) | (true, Some(true)) => Decision::Done(SaveChoice::Named(name.to_string())),
        (true, None) => Decision::AskReplace(name.to_string()),
        (true, Some(false)) => Decision::EditName(name.to_string()),
    }
}

/// The first dialog, with `name` in its field.
fn save_dialog(area: &MarkedArea, name: &str) -> (adw::AlertDialog, adw::EntryRow) {
    let body = gettext_f(
        "Top left at {x}, {y}",
        &[("x", &area.x.to_string()), ("y", &area.y.to_string())],
    );
    let dialog = adw::AlertDialog::new(Some(&area.describe()), Some(&body));
    dialog.add_responses(&[
        ("remark", &gettext("Mark Again")),
        ("done", &gettext("Done")),
    ]);
    dialog.set_response_appearance("done", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("done"));
    dialog.set_close_response("done");

    let entry_row = adw::EntryRow::builder()
        .title(gettext("Save as…"))
        .text(name)
        .activates_default(true)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.append(&entry_row);
    dialog.set_extra_child(Some(&list));

    (dialog, entry_row)
}

/// «There is already an area called “name”»: change the name or replace it.
fn replace_dialog(name: &str, area: &MarkedArea) -> adw::AlertDialog {
    let heading = gettext_f(
        "There is already an area called “{name}”",
        &[("name", name)],
    );
    let body = gettext_f(
        "Replace it with this one, {area}?",
        &[("area", &area.describe())],
    );
    let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
    dialog.add_responses(&[
        ("change", &gettext("Change Name")),
        ("replace", &gettext("Replace")),
    ]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("change"));
    dialog.set_close_response("change");
    dialog
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> MarkedArea {
        MarkedArea {
            connector: "DP-3".into(),
            x: 100,
            y: 80,
            width: 1280,
            height: 720,
            monitor_width: 1680,
            monitor_height: 1050,
            scale: 1.05,
        }
    }

    #[test]
    fn no_name_keeps_the_last_area() {
        assert_eq!(
            choice_for("", false, None),
            Decision::Done(SaveChoice::Last)
        );
        assert_eq!(
            choice_for("   ", true, None),
            Decision::Done(SaveChoice::Last)
        );
    }

    #[test]
    fn a_new_name_is_saved_trimmed() {
        assert_eq!(
            choice_for("  Demo ", false, None),
            Decision::Done(SaveChoice::Named("Demo".into()))
        );
    }

    #[test]
    fn a_taken_name_asks_then_replaces_or_goes_back() {
        assert_eq!(
            choice_for(" Demo", true, None),
            Decision::AskReplace("Demo".into())
        );
        assert_eq!(
            choice_for("Demo", true, Some(true)),
            Decision::Done(SaveChoice::Named("Demo".into()))
        );
        assert_eq!(
            choice_for("Demo", true, Some(false)),
            Decision::EditName("Demo".into())
        );
    }

    #[test]
    fn area_is_described_by_size_and_monitor() {
        assert_eq!(area().describe(), "1280×720 on DP-3");
    }

    /// Built, never presented.
    #[gtk::test]
    fn save_dialog_offers_a_name_and_done() {
        adw::init().unwrap();

        let (dialog, entry_row) = save_dialog(&area(), "Demo");
        assert_eq!(dialog.heading().as_deref(), Some("1280×720 on DP-3"));
        assert_eq!(dialog.body(), "Top left at 100, 80");
        assert_eq!(entry_row.text(), "Demo");
        assert!(entry_row.activates_default());
        assert!(dialog.has_response("remark"));
        assert_eq!(dialog.response_label("done"), "Done");
        assert_eq!(dialog.default_response().as_deref(), Some("done"));
        assert_eq!(dialog.close_response(), "done");
        assert_eq!(
            dialog.response_appearance("done"),
            adw::ResponseAppearance::Suggested
        );
    }

    /// Built, never presented.
    #[gtk::test]
    fn replace_dialog_offers_to_change_the_name() {
        adw::init().unwrap();

        let dialog = replace_dialog("Demo", &area());
        assert_eq!(
            dialog.heading().as_deref(),
            Some("There is already an area called “Demo”")
        );
        assert_eq!(dialog.response_label("change"), "Change Name");
        assert_eq!(
            dialog.response_appearance("replace"),
            adw::ResponseAppearance::Destructive
        );
        assert_eq!(dialog.default_response().as_deref(), Some("change"));
        assert_eq!(dialog.close_response(), "change");
    }
}
