#![allow(clippy::new_without_default)]
#![warn(
    rust_2018_idioms,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::explicit_iter_loop,
    clippy::semicolon_if_nothing_returned,
    clippy::match_wildcard_for_single_variants,
    clippy::inefficient_to_string,
    clippy::map_unwrap_or,
    clippy::implicit_clone,
    clippy::struct_excessive_bools,
    clippy::trivially_copy_pass_by_ref,
    clippy::unreadable_literal,
    clippy::if_not_else,
    clippy::doc_markdown,
    clippy::unused_async,
    clippy::default_trait_access,
    clippy::unnecessary_wraps,
    clippy::unused_self,
    clippy::dbg_macro,
    clippy::todo,
    clippy::print_stdout
)]

mod about;
mod application;
mod area_border;
mod area_marker;
mod area_selector;
mod areas;
mod camera_pip;
mod cancelled;
mod capture_warnings;
mod config;
mod device;
mod doctor;
mod experimental;
mod format;
mod help;
mod i18n;
mod item_row;
mod overlay;
mod pause_gate;
mod pipeline;
mod preferences_dialog;
mod profile;
mod recording;
mod screencast_portal;
mod settings;
mod timer;
mod tray;
mod window;

use std::{env, ffi::OsString};

use gettextrs::{LocaleCategory, gettext};
use gtk::{gio, glib, prelude::*};

use self::{
    application::Application,
    config::{GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_FILE},
    settings::Settings,
};

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt::init();

    gettextrs::setlocale(LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain.");
    gettextrs::textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain.");

    glib::set_application_name(&gettext("Kooha"));

    gst::init().expect("Unable to start gstreamer.");
    gstgif::plugin_register_static().expect("Failed to register gif plugin.");
    gstgtk4::plugin_register_static().expect("Failed to register gtk4 plugin.");

    let res = gio::Resource::load(RESOURCES_FILE).expect("Could not load gresource file.");
    gio::resources_register(&res);

    if check_requested(env::args_os()) {
        return run_check();
    }

    let app = Application::new();
    // Handled above, before GTK starts; registered so that `--help` lists it.
    app.add_main_option(
        "check",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        &gettext("Print the requirements report and exit"),
        None,
    );
    app.run()
}

/// Whether `kooha --check` was asked for. `args` includes the program name.
fn check_requested(args: impl IntoIterator<Item = OsString>) -> bool {
    args.into_iter().skip(1).any(|arg| arg == "--check")
}

/// `kooha --check`: prints the requirements report without opening any
/// window, and fails when a check failed.
#[allow(clippy::print_stdout)]
fn run_check() -> glib::ExitCode {
    // Without a display GTK does not start, and `overlay::availability()`
    // then says why overlays cannot be checked.
    if let Err(err) = gtk::init() {
        tracing::debug!("Checking without GTK: {:?}", err);
    }

    let settings = Settings::default();
    let report =
        glib::MainContext::default().block_on(doctor::run(&settings, overlay::availability()));
    print!("{}", report.to_text());

    if report.has_failures() {
        glib::ExitCode::FAILURE
    } else {
        glib::ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn check_flag() {
        assert!(check_requested(args(&["kooha", "--check"])));
        assert!(check_requested(args(&["kooha", "-v", "--check"])));
        assert!(!check_requested(args(&["kooha"])));
        assert!(!check_requested(args(&["kooha", "--checks"])));
        // The program name itself does not count.
        assert!(!check_requested(args(&["--check"])));
    }
}
