//! Screen overlays (camera `PiP`, area border) as `gtk4-layer-shell` surfaces.
//!
//! Without the `layer-shell` feature only `availability()` exists, and it
//! always reports `NotCompiled` (ADR 0001).

#[cfg(feature = "layer-shell")]
use std::cell::OnceCell;

#[cfg(feature = "layer-shell")]
use gtk::{gdk, prelude::*};
#[cfg(feature = "layer-shell")]
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

/// Why overlays (camera `PiP`, area border) cannot be shown, if they cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayUnavailable {
    // Each build constructs only the variants it can report.
    #[cfg_attr(feature = "layer-shell", allow(dead_code))]
    NotCompiled,
    #[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
    CompositorUnsupported,
    /// Compiled in, but there is no display to ask the compositor (`kooha --check`
    /// from a plain terminal). Only the requirements report uses it.
    #[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
    NoDisplay,
}

#[cfg(feature = "layer-shell")]
thread_local! {
    /// The answer for the default display, which does not change while the
    /// process runs. The tray asks once per second while recording.
    static AVAILABILITY: OnceCell<Result<(), OverlayUnavailable>> = const { OnceCell::new() };
}

/// `NotCompiled` is decided first, without touching GTK, so it is reported even
/// without a display (codex r1 #12).
pub fn availability() -> Result<(), OverlayUnavailable> {
    #[cfg(not(feature = "layer-shell"))]
    {
        Err(OverlayUnavailable::NotCompiled)
    }

    #[cfg(feature = "layer-shell")]
    {
        if !gtk::is_initialized_main_thread() {
            return Err(OverlayUnavailable::NoDisplay);
        }
        // Not cached: GTK may get a display later in the process.
        let Some(display) = gdk::Display::default() else {
            return Err(OverlayUnavailable::NoDisplay);
        };

        AVAILABILITY.with(|cached| *cached.get_or_init(|| availability_on(&display)))
    }
}

/// Layer surfaces exist only on Wayland, and on any other display
/// `gtk4_layer_shell::is_supported()` fails an assertion, so it is never
/// asked there (codex code r1 #7, fable code r1 #3).
#[cfg(feature = "layer-shell")]
fn availability_on(display: &gdk::Display) -> Result<(), OverlayUnavailable> {
    decide(
        display.backend().is_wayland(),
        gtk4_layer_shell::is_supported,
    )
}

#[cfg(feature = "layer-shell")]
fn decide(
    is_wayland: bool,
    layer_shell_supported: impl FnOnce() -> bool,
) -> Result<(), OverlayUnavailable> {
    if is_wayland && layer_shell_supported() {
        Ok(())
    } else {
        Err(OverlayUnavailable::CompositorUnsupported)
    }
}

/// Turns `window` into an overlay layer surface on `monitor`, anchored to its
/// top-left corner.
///
/// Only callable when `availability()` is Ok, and before the window is
/// realized.
#[cfg(feature = "layer-shell")]
pub fn init_overlay(window: &gtk::Window, monitor: &gdk::Monitor, namespace: &str) {
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some(namespace));
    window.set_monitor(Some(monitor));
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(-1);
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
}

/// Places a top-left anchored overlay at `(left, top)` of its monitor.
#[cfg(feature = "layer-shell")]
pub fn set_margins(window: &gtk::Window, left: i32, top: i32) {
    window.set_margin(Edge::Left, left);
    window.set_margin(Edge::Top, top);
}

/// Stretches an overlay over its whole monitor.
#[cfg(feature = "layer-shell")]
// The area border (chunk 5) is the first user.
#[allow(dead_code)]
pub fn anchor_all_edges(window: &gtk::Window) {
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
}

#[cfg(all(test, not(feature = "layer-shell")))]
mod tests {
    use super::*;

    #[test]
    fn availability_without_feature() {
        assert_eq!(availability(), Err(OverlayUnavailable::NotCompiled));
    }
}

#[cfg(all(test, feature = "layer-shell"))]
mod tests {
    use std::{
        path::Path,
        process::{Child, Command},
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn decide_never_asks_layer_shell_off_wayland() {
        assert_eq!(
            decide(false, || panic!("asked layer-shell on X11")),
            Err(OverlayUnavailable::CompositorUnsupported)
        );
        assert_eq!(decide(true, || true), Ok(()));
        assert_eq!(
            decide(true, || false),
            Err(OverlayUnavailable::CompositorUnsupported)
        );
    }

    /// An Xvfb of its own, killed on drop.
    struct Xvfb {
        child: Child,
        name: String,
    }

    impl Xvfb {
        fn start() -> Option<Self> {
            let number = (90..130).find(|n| {
                !Path::new(&format!("/tmp/.X11-unix/X{n}")).exists()
                    && !Path::new(&format!("/tmp/.X{n}-lock")).exists()
            })?;
            let name = format!(":{number}");
            let child = Command::new("Xvfb")
                .args([
                    name.as_str(),
                    "-nolisten",
                    "tcp",
                    "-screen",
                    "0",
                    "640x480x24",
                ])
                .spawn()
                .ok()?;
            let xvfb = Self { child, name };

            let socket = format!("/tmp/.X11-unix/X{number}");
            let started = Instant::now();
            while !Path::new(&socket).exists() {
                if started.elapsed() > Duration::from_secs(5) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Some(xvfb)
        }
    }

    impl Drop for Xvfb {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// On X11 `gtk4_layer_shell::is_supported()` fails an assertion, a
    /// `CRITICAL` per call (codex code r1 #7, fable code r1 #3).
    #[gtk::test]
    fn x11_display_is_unsupported() {
        let Some(xvfb) = Xvfb::start() else {
            eprintln!("Skipped: cannot start Xvfb");
            return;
        };
        let display = gdk::Display::open(Some(&xvfb.name)).expect("the Xvfb display");
        assert!(!display.backend().is_wayland());

        assert_eq!(
            availability_on(&display),
            Err(OverlayUnavailable::CompositorUnsupported)
        );

        display.close();
    }
}
