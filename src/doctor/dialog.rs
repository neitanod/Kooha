//! The "Requirements Report" dialog: one row per check, with the problem, its
//! impact and the fix only under the failures and warnings.

use adw::{prelude::*, subclass::prelude::*};
use gettextrs::gettext;
use gtk::glib::{self, clone};

use super::{CheckResult, Failure, Report, Status};
use crate::{overlay, settings::Settings};

mod imp {
    use std::cell::RefCell;

    use super::*;

    #[derive(Debug, Default)]
    pub struct RequirementsDialog {
        pub(super) stack: gtk::Stack,
        pub(super) list_box: gtk::ListBox,
        pub(super) copy_report_button: gtk::Button,
        pub(super) report_text: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RequirementsDialog {
        const NAME: &'static str = "KoohaRequirementsDialog";
        type Type = super::RequirementsDialog;
        type ParentType = adw::Dialog;
    }

    impl ObjectImpl for RequirementsDialog {
        fn constructed(&self) {
            self.parent_constructed();

            self.obj().build();
        }
    }

    impl WidgetImpl for RequirementsDialog {}
    impl AdwDialogImpl for RequirementsDialog {}
}

glib::wrapper! {
    pub struct RequirementsDialog(ObjectSubclass<imp::RequirementsDialog>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl RequirementsDialog {
    /// A dialog that shows a spinner until `set_report` is called.
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Opens the dialog over `parent` and runs the checks.
    pub fn present_and_run(parent: &impl IsA<gtk::Widget>, settings: &Settings) {
        let dialog = Self::new();
        dialog.present(Some(parent));

        let settings = settings.clone();
        glib::spawn_future_local(clone!(
            #[weak]
            dialog,
            async move {
                let report = super::run(&settings, overlay::availability()).await;
                dialog.set_report(&report);
            }
        ));
    }

    /// Replaces the rows with the checks of `report` and shows them.
    pub fn set_report(&self, report: &Report) {
        let imp = self.imp();

        imp.list_box.remove_all();
        for check in &report.0 {
            imp.list_box.append(&check_row(check));
        }

        imp.report_text.replace(report.to_text());
        imp.copy_report_button.set_sensitive(true);
        imp.stack.set_visible_child_name("report");
    }

    fn build(&self) {
        let imp = self.imp();

        self.set_title(&gettext("Requirements Report"));
        self.set_content_width(520);
        self.set_content_height(640);

        let spinner = adw::Spinner::builder()
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        imp.stack.add_named(&spinner, Some("loading"));

        imp.list_box.set_selection_mode(gtk::SelectionMode::None);
        imp.list_box.set_valign(gtk::Align::Start);
        imp.list_box.add_css_class("boxed-list");
        let clamp = adw::Clamp::builder()
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .child(&imp.list_box)
            .build();
        let scrolled_window = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        imp.stack.add_named(&scrolled_window, Some("report"));
        imp.stack.set_visible_child_name("loading");

        imp.copy_report_button.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("edit-copy-symbolic")
                .label(gettext("Copy Report"))
                .build(),
        ));
        imp.copy_report_button.add_css_class("pill");
        imp.copy_report_button.set_halign(gtk::Align::Center);
        imp.copy_report_button.set_margin_top(6);
        imp.copy_report_button.set_margin_bottom(12);
        imp.copy_report_button.set_sensitive(false);
        imp.copy_report_button.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |button| {
                button.clipboard().set_text(&obj.imp().report_text.borrow());
                button.add_css_class("copy-done");
                button.set_tooltip_text(Some(&gettext("Copied to clipboard")));
            }
        ));

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&adw::HeaderBar::new());
        toolbar_view.set_content(Some(&imp.stack));
        toolbar_view.add_bottom_bar(&imp.copy_report_button);
        self.set_child(Some(&toolbar_view));
    }
}

fn check_row(check: &CheckResult) -> gtk::ListBoxRow {
    let (icon_name, css_class) = match check.status {
        Status::Ok => ("object-select-symbolic", "success"),
        Status::Warning => ("dialog-warning-symbolic", "warning"),
        Status::Failed => ("window-close-symbolic", "error"),
    };
    let icon = gtk::Image::builder()
        .name("check-status")
        .icon_name(icon_name)
        .valign(gtk::Align::Center)
        .build();
    icon.add_css_class(css_class);

    let title = wrapped_label("check-title", &check.title);
    title.add_css_class("heading");
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 3);
    titles.append(&title);
    if let Some(detail) = &check.detail {
        let detail = wrapped_label("check-detail", detail);
        detail.add_css_class("dim-label");
        detail.add_css_class("caption");
        titles.append(&detail);
    }

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.append(&icon);
    header.append(&titles);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&header);

    if let Some(failure) = check
        .failure
        .as_ref()
        .filter(|_| check.status != Status::Ok)
    {
        content.append(&failure_box(failure));
    }

    gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&content)
        .build()
}

/// Problem, impact and fix, under the header of a failed or warned check.
fn failure_box(failure: &Failure) -> gtk::Box {
    let failure_box = gtk::Box::builder()
        .name("check-failure")
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

    failure_box.append(&heading_label(&gettext("Problem")));
    failure_box.append(&wrapped_label("problem", &failure.problem));
    failure_box.append(&heading_label(&gettext("What stops working")));
    failure_box.append(&wrapped_label("impact", &failure.impact));

    if failure.fix_command.is_none() && failure.fix_note.is_none() {
        return failure_box;
    }

    failure_box.append(&heading_label(&gettext("How to fix it")));

    if let Some(command) = &failure.fix_command {
        let command_label = wrapped_label("fix-command", command);
        command_label.set_selectable(true);
        command_label.set_hexpand(true);
        command_label.add_css_class("monospace");

        let command_text = command.clone();
        let copy_button = gtk::Button::builder()
            .name("copy-fix-command")
            .valign(gtk::Align::Center)
            .tooltip_text(gettext("Copy to clipboard"))
            .icon_name("edit-copy-symbolic")
            .build();
        copy_button.add_css_class("flat");
        copy_button.connect_clicked(move |button| {
            button.clipboard().set_text(&command_text);
            button.set_tooltip_text(Some(&gettext("Copied to clipboard")));
            button.set_icon_name("checkmark-symbolic");
            button.add_css_class("copy-done");
        });

        let command_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        command_box.append(&command_label);
        command_box.append(&copy_button);
        failure_box.append(&command_box);
    }

    if let Some(note) = &failure.fix_note {
        failure_box.append(&wrapped_label("fix-note", note));
    }

    failure_box
}

fn heading_label(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("caption-heading");
    label
}

fn wrapped_label(name: &str, text: &str) -> gtk::Label {
    gtk::Label::builder()
        .name(name)
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(title: &str, status: Status, fix_command: Option<&str>) -> CheckResult {
        let failure = (status != Status::Ok).then(|| Failure {
            problem: format!("{title} problem"),
            impact: format!("{title} impact"),
            fix_command: fix_command.map(str::to_string),
            fix_note: Some(format!("{title} note")),
        });

        CheckResult {
            title: title.to_string(),
            status,
            detail: Some(format!("{title} detail")),
            failure,
        }
    }

    fn report() -> Report {
        Report(vec![
            check("A", Status::Ok, None),
            check("B", Status::Warning, None),
            check("C", Status::Failed, Some("sudo apt install c")),
        ])
    }

    /// The descendants of `widget` named `name`, depth first.
    fn named(widget: &gtk::Widget, name: &str) -> Vec<gtk::Widget> {
        let mut found = Vec::new();
        let mut child = widget.first_child();
        while let Some(current) = child {
            if current.widget_name() == name {
                found.push(current.clone());
            }
            found.extend(named(&current, name));
            child = current.next_sibling();
        }
        found
    }

    fn label_text(widget: &gtk::Widget) -> String {
        widget.downcast_ref::<gtk::Label>().unwrap().label().into()
    }

    fn rows(dialog: &RequirementsDialog) -> Vec<gtk::Widget> {
        let list_box = &dialog.imp().list_box;
        (0..)
            .map_while(|index| list_box.row_at_index(index))
            .map(|row| row.upcast())
            .collect()
    }

    #[gtk::test]
    fn shows_spinner_until_report() {
        let dialog = RequirementsDialog::new();
        let imp = dialog.imp();

        assert_eq!(dialog.title(), "Requirements Report");
        assert_eq!(imp.stack.visible_child_name().as_deref(), Some("loading"));
        assert!(!imp.copy_report_button.is_sensitive());

        dialog.set_report(&report());

        assert_eq!(imp.stack.visible_child_name().as_deref(), Some("report"));
        assert!(imp.copy_report_button.is_sensitive());
        assert_eq!(*imp.report_text.borrow(), report().to_text());
    }

    #[gtk::test]
    fn one_row_per_check_with_status_icon() {
        let dialog = RequirementsDialog::new();
        dialog.set_report(&report());

        let rows = rows(&dialog);
        assert_eq!(rows.len(), 3);

        let expected = [
            ("object-select-symbolic", "success"),
            ("dialog-warning-symbolic", "warning"),
            ("window-close-symbolic", "error"),
        ];
        for (row, (icon_name, class)) in rows.iter().zip(expected) {
            let icons = named(row, "check-status");
            assert_eq!(icons.len(), 1);
            let icon = icons[0].downcast_ref::<gtk::Image>().unwrap();
            assert_eq!(icon.icon_name().as_deref(), Some(icon_name));
            assert!(icon.has_css_class(class));
        }

        assert_eq!(label_text(&named(&rows[0], "check-title")[0]), "A");
        assert_eq!(label_text(&named(&rows[0], "check-detail")[0]), "A detail");
    }

    #[gtk::test]
    fn failure_details_only_under_warnings_and_failures() {
        let dialog = RequirementsDialog::new();
        dialog.set_report(&report());

        let rows = rows(&dialog);
        assert!(named(&rows[0], "check-failure").is_empty());
        assert_eq!(named(&rows[1], "check-failure").len(), 1);
        assert_eq!(named(&rows[2], "check-failure").len(), 1);

        // The warning has no command, only the note.
        assert!(named(&rows[1], "fix-command").is_empty());
        assert!(named(&rows[1], "copy-fix-command").is_empty());
        assert_eq!(label_text(&named(&rows[1], "fix-note")[0]), "B note");

        let commands = named(&rows[2], "fix-command");
        assert_eq!(commands.len(), 1);
        assert_eq!(label_text(&commands[0]), "sudo apt install c");
        assert_eq!(named(&rows[2], "copy-fix-command").len(), 1);
        assert_eq!(label_text(&named(&rows[2], "problem")[0]), "C problem");
        assert_eq!(label_text(&named(&rows[2], "impact")[0]), "C impact");
    }

    #[gtk::test]
    fn set_report_replaces_the_rows() {
        let dialog = RequirementsDialog::new();
        dialog.set_report(&report());
        dialog.set_report(&Report(vec![check("A", Status::Ok, None)]));

        assert_eq!(rows(&dialog).len(), 1);
    }
}
