//! The Areas window: the saved areas, to choose, rename, mark again and
//! delete them.

use std::{cell::Cell, rc::Rc};

use adw::{prelude::*, subclass::prelude::*};
use gettextrs::gettext;
use gtk::{
    gdk,
    glib::{self, clone},
};

use crate::{
    areas::{AreaStore, MonitorInfo, SavedArea, Selected, monitors},
    i18n::gettext_f,
};

/// What to mark, as one of the two named actions of the main window. Only
/// the main window marks areas, so everyone else asks it this way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkRequest {
    /// A new area: `win.mark-area`.
    New,
    /// The saved area of this name, again: `win.remark-area`.
    Again(String),
}

impl MarkRequest {
    fn action(&self) -> (&'static str, Option<glib::Variant>) {
        match self {
            Self::New => ("win.mark-area", None),
            Self::Again(name) => ("win.remark-area", Some(name.to_variant())),
        }
    }

    /// Activates the action on `widget`, normally the main window.
    pub fn activate_on(&self, widget: &impl IsA<gtk::Widget>) {
        let (name, target) = self.action();
        if let Err(err) = WidgetExt::activate_action(widget, name, target.as_ref()) {
            tracing::warn!("Failed to activate `{}`: {:?}", name, err);
        }
    }
}

/// What a row of the window shows for one saved area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowModel {
    pub title: String,
    /// The area, or why it is out of date.
    pub subtitle: String,
    pub stale: bool,
    pub can_choose: bool,
    pub can_mark_again: bool,
}

/// The row of `area`, chosen or not, with the connected `monitors`.
pub fn row_model(area: &SavedArea, chosen: bool, monitors: &[MonitorInfo]) -> RowModel {
    let staleness = area.area.staleness(monitors);
    let description = staleness.as_ref().map_or_else(
        || area.area.describe(),
        |staleness| staleness.describe(&area.area.connector),
    );
    let subtitle = if chosen {
        gettext_f("{area} · chosen", &[("area", &description)])
    } else {
        description
    };

    RowModel {
        title: area.name.clone(),
        subtitle,
        stale: staleness.is_some(),
        can_choose: !chosen && staleness.is_none(),
        // Also with the monitor gone: marking again starts from scratch.
        can_mark_again: true,
    }
}

/// How renaming an area from `from` to `to` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameOutcome {
    /// Same or empty name: nothing to do.
    Unchanged,
    Renamed,
    /// Another area has the name; the text explains it.
    Taken(String),
}

/// Renames the saved area `from` to `to`, without surrounding spaces.
pub fn rename(store: &AreaStore, from: &str, to: &str) -> RenameOutcome {
    let to = to.trim();
    if to.is_empty() || to == from {
        return RenameOutcome::Unchanged;
    }

    match store.rename(from, to) {
        Ok(()) => RenameOutcome::Renamed,
        Err(taken) => RenameOutcome::Taken(gettext_f(
            "There is already an area called “{name}”",
            &[("name", &taken.0)],
        )),
    }
}

mod imp {
    use std::cell::{Cell, OnceCell, RefCell};

    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/seadve/Kooha/ui/areas_window.ui")]
    pub struct AreasWindow {
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) mark_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) list_box: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) footer_label: TemplateChild<gtk::Label>,

        pub(super) store: OnceCell<AreaStore>,
        pub(super) store_handler: RefCell<Option<glib::SignalHandlerId>>,
        /// The main window ignores marking and choosing while recording.
        pub(super) recording: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AreasWindow {
        const NAME: &'static str = "KoohaAreasWindow";
        type Type = super::AreasWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_instance_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AreasWindow {
        fn dispose(&self) {
            if let (Some(store), Some(handler)) = (self.store.get(), self.store_handler.take()) {
                store.disconnect(handler);
            }
        }
    }

    impl WidgetImpl for AreasWindow {}
    impl WindowImpl for AreasWindow {}
    impl AdwWindowImpl for AreasWindow {}
}

glib::wrapper! {
    pub struct AreasWindow(ObjectSubclass<imp::AreasWindow>)
        @extends gtk::Widget, gtk::Window, adw::Window,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

#[gtk::template_callbacks]
impl AreasWindow {
    /// The window of the areas of `store`, over `parent`, the main window,
    /// whose actions mark the areas.
    pub fn new(store: &AreaStore, parent: &impl IsA<gtk::Window>) -> Self {
        let obj = glib::Object::builder::<Self>()
            .property("transient-for", parent)
            .build();
        let imp = obj.imp();

        imp.store.set(store.clone()).unwrap();
        imp.footer_label.set_label(&gettext_f(
            "Saved in {path}",
            &[("path", &store.path().display().to_string())],
        ));

        let handler = store.connect_changed(clone!(
            #[weak]
            obj,
            move |_| obj.rebuild()
        ));
        imp.store_handler.replace(Some(handler));

        // Areas go out of date, and back, as monitors change.
        monitors::watch(
            &WidgetExt::display(&obj),
            clone!(
                #[weak]
                obj,
                move || obj.rebuild()
            ),
        );

        obj.rebuild();
        obj
    }

    /// Disables choosing and marking while a recording goes on.
    pub fn set_recording(&self, recording: bool) {
        let imp = self.imp();
        if imp.recording.replace(recording) == recording {
            return;
        }

        imp.mark_button.set_sensitive(!recording);
        self.rebuild();
    }

    pub fn is_recording(&self) -> bool {
        self.imp().recording.get()
    }

    fn store(&self) -> &AreaStore {
        self.imp().store.get().unwrap()
    }

    #[template_callback]
    fn mark_new_area(&self) {
        self.ask_main_window(&MarkRequest::New);
    }

    fn ask_main_window(&self, request: &MarkRequest) {
        match self.transient_for() {
            Some(main_window) => request.activate_on(&main_window),
            None => tracing::warn!("No main window to ask for {:?}", request),
        }
    }

    fn rebuild(&self) {
        let imp = self.imp();
        let store = self.store();

        imp.list_box.remove_all();

        let areas = store.areas();
        imp.stack
            .set_visible_child_name(if areas.is_empty() { "empty" } else { "list" });

        let monitors = monitors::current(&WidgetExt::display(self));
        let selected = store.selected();
        for area in &areas {
            let chosen = selected.as_ref() == Some(&Selected::Named(area.name.clone()));
            let model = row_model(area, chosen, &monitors);
            imp.list_box.append(&self.build_row(&area.name, &model));
        }
    }

    fn build_row(&self, name: &str, model: &RowModel) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&model.title)
            .subtitle(&model.subtitle)
            .use_markup(false)
            .build();

        if model.stale {
            let icon = gtk::Image::from_icon_name("warning-symbolic");
            icon.add_css_class("warning");
            icon.set_tooltip_text(Some(&model.subtitle));
            row.add_prefix(&icon);
        }

        let editable = gtk::EditableLabel::builder()
            .visible(false)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        row.add_prefix(&editable);
        self.connect_rename(&row, &editable, name);

        let choose = gtk::Button::builder()
            .name("choose")
            .label(gettext("Choose"))
            .sensitive(model.can_choose && !self.is_recording())
            .valign(gtk::Align::Center)
            .build();
        choose.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            #[to_owned]
            name,
            move |_| obj.store().select(Some(Selected::Named(name.clone())))
        ));
        row.add_suffix(&choose);

        let rename = icon_button("rename", "document-edit-symbolic", &gettext("Rename"));
        rename.connect_clicked(clone!(
            #[weak]
            row,
            #[weak]
            editable,
            #[to_owned]
            name,
            move |_| start_rename(&row, &editable, &name)
        ));
        row.add_suffix(&rename);

        let mark_again = icon_button("mark-again", "selection-symbolic", &gettext("Mark Again"));
        mark_again.set_sensitive(model.can_mark_again && !self.is_recording());
        mark_again.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            #[to_owned]
            name,
            move |_| obj.ask_main_window(&MarkRequest::Again(name.clone()))
        ));
        row.add_suffix(&mark_again);

        let delete = icon_button("delete", "user-trash-symbolic", &gettext("Delete"));
        delete.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            #[to_owned]
            name,
            move |_| obj.confirm_delete(&name)
        ));
        row.add_suffix(&delete);

        row
    }

    /// Enter renames, Escape cancels, and a taken name keeps the field in
    /// edition, marked as an error.
    fn connect_rename(&self, row: &adw::ActionRow, editable: &gtk::EditableLabel, name: &str) {
        // `notify::editing` can't tell Enter from Escape, so remember it.
        let cancelled = Rc::new(Cell::new(false));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(clone!(
            #[strong]
            cancelled,
            move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    cancelled.set(true);
                }
                glib::Propagation::Proceed
            }
        ));
        editable.add_controller(keys);

        editable.connect_editing_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            row,
            #[to_owned]
            name,
            move |editable| {
                if editable.is_editing() || !editable.is_visible() {
                    return;
                }
                if cancelled.replace(false) {
                    finish_rename(&row, editable, &name);
                    return;
                }
                match rename(obj.store(), &name, &editable.text()) {
                    RenameOutcome::Unchanged | RenameOutcome::Renamed => {
                        finish_rename(&row, editable, &name);
                    }
                    RenameOutcome::Taken(message) => {
                        editable.add_css_class("error");
                        editable.set_tooltip_text(Some(&message));
                        editable.start_editing();
                    }
                }
            }
        ));
    }

    fn confirm_delete(&self, name: &str) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext_f("Delete “{name}”?", &[("name", name)])),
            Some(&gettext("The area is removed from the list.")),
        );
        dialog.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("delete", &gettext("Delete")),
        ]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            Some("delete"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                #[to_owned]
                name,
                move |_, _| obj.store().remove(&name)
            ),
        );
        dialog.present(Some(self));
    }
}

#[cfg(test)]
impl AreasWindow {
    fn shows_empty_state(&self) -> bool {
        self.imp().stack.visible_child_name().as_deref() == Some("empty")
    }

    fn row(&self, index: i32) -> Option<adw::ActionRow> {
        self.imp()
            .list_box
            .row_at_index(index)
            .and_downcast::<adw::ActionRow>()
    }

    fn row_count(&self) -> i32 {
        let mut count = 0;
        while self.imp().list_box.row_at_index(count).is_some() {
            count += 1;
        }
        count
    }

    fn footer_text(&self) -> String {
        self.imp().footer_label.label().to_string()
    }
}

fn icon_button(name: &str, icon_name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .name(name)
        .icon_name(icon_name)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .build();
    button.add_css_class("flat");
    button
}

/// Shows the name of the row as an editable field, in edition.
fn start_rename(row: &adw::ActionRow, editable: &gtk::EditableLabel, name: &str) {
    row.set_title("");
    editable.set_text(name);
    editable.set_visible(true);
    editable.start_editing();
    editable.grab_focus();
}

fn finish_rename(row: &adw::ActionRow, editable: &gtk::EditableLabel, name: &str) {
    editable.remove_css_class("error");
    editable.set_tooltip_text(None);
    editable.set_visible(false);
    row.set_title(name);
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gtk::{gio, glib, prelude::*};

    use super::*;
    use crate::areas::{AreaStore, MarkedArea, MonitorInfo, SavedArea};

    fn dp3() -> MonitorInfo {
        MonitorInfo {
            connector: "DP-3".into(),
            logical: (1920, 454, 1600, 1000),
            scale: 1.05,
        }
    }

    fn saved(name: &str) -> SavedArea {
        SavedArea {
            name: name.into(),
            area: MarkedArea {
                connector: "DP-3".into(),
                x: 100,
                y: 80,
                width: 1280,
                height: 720,
                monitor_width: 1680,
                monitor_height: 1050,
                scale: 1.05,
            },
        }
    }

    #[test]
    fn fresh_row_not_chosen() {
        let row = row_model(&saved("Demo"), false, &[dp3()]);

        assert_eq!(
            row,
            RowModel {
                title: "Demo".into(),
                subtitle: "1280×720 on DP-3".into(),
                stale: false,
                can_choose: true,
                can_mark_again: true,
            }
        );
    }

    #[test]
    fn chosen_row_cannot_be_chosen_again() {
        let row = row_model(&saved("Demo"), true, &[dp3()]);

        assert_eq!(row.subtitle, "1280×720 on DP-3 · chosen");
        assert!(!row.stale);
        assert!(!row.can_choose);
        assert!(row.can_mark_again);
    }

    #[test]
    fn disconnected_row_says_why_and_can_be_marked_again() {
        let row = row_model(&saved("Demo"), false, &[]);

        assert_eq!(row.subtitle, "DP-3 is not connected");
        assert!(row.stale);
        assert!(!row.can_choose);
        assert!(row.can_mark_again);
    }

    #[test]
    fn rescaled_row_says_why_and_can_be_marked_again() {
        let mut monitor = dp3();
        monitor.scale = 1.25;
        monitor.logical = (1920, 454, 1344, 840); // same physical size

        let row = row_model(&saved("Demo"), true, &[monitor]);

        assert_eq!(
            row.subtitle,
            "DP-3 changed scale from 1.05 to 1.25 · chosen"
        );
        assert!(row.stale);
        assert!(!row.can_choose);
        assert!(row.can_mark_again);
    }

    #[test]
    fn invalid_row_says_why_and_can_be_marked_again() {
        let mut edited = saved("Edited");
        edited.area.x = -2;

        let row = row_model(&edited, false, &[dp3()]);

        assert_eq!(row.subtitle, "the area starts outside DP-3");
        assert!(row.stale);
        assert!(!row.can_choose);
        assert!(row.can_mark_again);
    }

    /// The actions `mark-area` and `remark-area` of a fake `win` group on
    /// `widget`, recording each activation with its parameter.
    fn fake_mark_actions(
        widget: &impl IsA<gtk::Widget>,
    ) -> Rc<RefCell<Vec<(String, Option<String>)>>> {
        let log = Rc::new(RefCell::new(Vec::new()));
        let group = gio::SimpleActionGroup::new();

        let mark = gio::SimpleAction::new("mark-area", None);
        let mark_log = log.clone();
        mark.connect_activate(move |_, param| {
            assert!(param.is_none());
            mark_log.borrow_mut().push(("mark-area".to_string(), None));
        });
        group.add_action(&mark);

        let remark = gio::SimpleAction::new("remark-area", Some(glib::VariantTy::STRING));
        let remark_log = log.clone();
        remark.connect_activate(move |_, param| {
            let name = param.and_then(|p| p.get::<String>());
            remark_log
                .borrow_mut()
                .push(("remark-area".to_string(), name));
        });
        group.add_action(&remark);

        widget.insert_action_group("win", Some(&group));
        log
    }

    #[gtk::test]
    fn mark_requests_activate_the_main_window_actions() {
        let target = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let log = fake_mark_actions(&target);

        MarkRequest::New.activate_on(&target);
        MarkRequest::Again("Demo".into()).activate_on(&target);

        assert_eq!(
            *log.borrow(),
            [
                ("mark-area".to_string(), None),
                ("remark-area".to_string(), Some("Demo".to_string())),
            ]
        );
    }

    fn store_with(names: &[&str]) -> AreaStore {
        let store = AreaStore::load_default();
        for name in names {
            store.save_named(name, saved(name).area, false).unwrap();
        }
        store
    }

    #[test]
    fn rename_outcomes() {
        let store = store_with(&["Demo", "Other"]);

        assert_eq!(rename(&store, "Demo", "Demo"), RenameOutcome::Unchanged);
        assert_eq!(rename(&store, "Demo", "   "), RenameOutcome::Unchanged);
        assert_eq!(
            rename(&store, "Demo", "Other"),
            RenameOutcome::Taken("There is already an area called “Other”".to_string())
        );
        assert!(store.contains("Demo"));

        assert_eq!(rename(&store, "Demo", " Talk "), RenameOutcome::Renamed);
        assert!(store.contains("Talk"));
        assert!(!store.contains("Demo"));
    }

    /// The first button named `name` under `widget`.
    fn find_button(widget: &gtk::Widget, name: &str) -> Option<gtk::Button> {
        if widget.widget_name() == name {
            if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                return Some(button.clone());
            }
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = find_button(&current, name) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }

    fn areas_window(
        store: &AreaStore,
    ) -> (AreasWindow, Rc<RefCell<Vec<(String, Option<String>)>>>) {
        let parent = gtk::Window::new();
        let log = fake_mark_actions(&parent);
        (AreasWindow::new(store, &parent), log)
    }

    #[gtk::test]
    fn empty_list_offers_to_mark_an_area() {
        crate::settings::test_utils::init_resources();
        let store = store_with(&[]);
        let (window, log) = areas_window(&store);

        assert!(window.shows_empty_state());
        find_button(window.upcast_ref(), "mark-area")
            .expect("empty state has a Mark Area button")
            .emit_clicked();

        assert_eq!(*log.borrow(), [("mark-area".to_string(), None)]);
    }

    #[gtk::test]
    fn rows_follow_the_store() {
        crate::settings::test_utils::init_resources();
        let store = store_with(&[]);
        let (window, _log) = areas_window(&store);

        store.save_named("Demo", saved("Demo").area, false).unwrap();
        store.save_named("Talk", saved("Talk").area, false).unwrap();
        assert!(!window.shows_empty_state());
        assert_eq!(window.row_count(), 2);

        store.remove("Demo");
        assert_eq!(window.row_count(), 1);
    }

    #[gtk::test]
    fn row_buttons_choose_and_mark_again() {
        crate::settings::test_utils::init_resources();
        let store = store_with(&["Demo", "Talk"]); // Talk is chosen
        let (window, log) = areas_window(&store);

        let first = window.row(0).expect("a row for Demo");
        find_button(first.upcast_ref(), "mark-again")
            .expect("row has Mark Again")
            .emit_clicked();
        assert_eq!(
            *log.borrow(),
            [("remark-area".to_string(), Some("Demo".to_string()))]
        );

        let choose = find_button(first.upcast_ref(), "choose").expect("row has Choose");
        choose.emit_clicked();
        assert_eq!(
            store.selected(),
            Some(crate::areas::Selected::Named("Demo".into()))
        );
    }

    /// While recording, the window's marking and choosing do nothing: the
    /// main window ignores its actions then.
    #[gtk::test]
    fn recording_disables_choosing_and_marking() {
        crate::settings::test_utils::init_resources();
        let store = store_with(&["Demo", "Talk"]); // Talk is chosen
        let (window, _log) = areas_window(&store);
        let sensitive = |name: &str| {
            find_button(window.row(0).unwrap().upcast_ref(), name)
                .unwrap()
                .is_sensitive()
        };
        let mark_area = || {
            find_button(window.upcast_ref(), "mark-area")
                .unwrap()
                .is_sensitive()
        };

        window.set_recording(true);
        assert!(!sensitive("choose"));
        assert!(!sensitive("mark-again"));
        assert!(sensitive("rename"));
        assert!(sensitive("delete"));
        assert!(!mark_area());

        // Also after the list changes.
        store
            .save_named("Other", saved("Other").area, false)
            .unwrap();
        store.select(Some(crate::areas::Selected::Named("Talk".into())));
        assert!(!sensitive("choose"));

        window.set_recording(false);
        assert!(sensitive("choose"));
        assert!(sensitive("mark-again"));
        assert!(mark_area());
    }

    #[gtk::test]
    fn footer_says_where_the_list_is_saved() {
        crate::settings::test_utils::init_resources();
        let store = store_with(&[]);
        let (window, _log) = areas_window(&store);

        assert_eq!(
            window.footer_text(),
            format!("Saved in {}", store.path().display())
        );
    }
}
