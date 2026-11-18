//! The marker: a layer per monitor where the area to record is dragged.
//!
//! Without the `layer-shell` feature only `mark()` exists, and it always
//! fails with `LayersUnavailable` (ADR 0004: the upstream selector stays).

#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
pub mod geometry;
#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
mod monitor;
#[cfg(feature = "layer-shell")]
mod view;

use std::{cell::Cell, error, fmt, future::Future, pin::pin, time::Duration};

use futures_util::future::{self, Either};
use gtk::{gdk, glib};

use self::geometry::Rect;
use crate::{
    areas::{MarkedArea, MonitorInfo},
    cancelled::Cancelled,
    overlay::OverlayUnavailable,
};

/// With this variable set, a marking still open after that many seconds
/// cancels itself: a last resort if the layers keep the keyboard.
#[cfg(feature = "layer-shell")]
const TIMEOUT_VARIABLE: &str = "KOOHA_MARKER_TIMEOUT_SECS";

/// The marker cannot show its layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayersUnavailable(pub OverlayUnavailable);

impl fmt::Display for LayersUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cannot mark an area on the screen: {:?}", self.0)
    }
}

impl error::Error for LayersUnavailable {}

/// Another marking is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyMarking;

impl fmt::Display for AlreadyMarking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "An area is already being marked")
    }
}

impl error::Error for AlreadyMarking {}

/// Shows a marking layer on every monitor and waits for the area: Enter
/// returns it, Escape or the secondary button fail with `Cancelled`. The
/// layers are destroyed before it returns.
///
/// `initial` is preloaded if its monitor is connected and the rectangle fits
/// on it.
// The window and the recording call it (chunk 5).
#[allow(dead_code)]
#[cfg_attr(not(feature = "layer-shell"), allow(clippy::unused_async))]
pub async fn mark(initial: Option<MarkedArea>) -> anyhow::Result<MarkedArea> {
    #[cfg(not(feature = "layer-shell"))]
    {
        let _ = initial;
        Err(LayersUnavailable(OverlayUnavailable::NotCompiled).into())
    }

    #[cfg(feature = "layer-shell")]
    {
        let _marking = begin_marking()?;
        crate::overlay::availability().map_err(LayersUnavailable)?;
        let display =
            gdk::Display::default().ok_or(LayersUnavailable(OverlayUnavailable::NoDisplay))?;

        let (marker, result) = AreaMarker::new(monitor::current(&display), initial.as_ref());
        marker.present();
        let result = with_timeout(result, timeout_from(std::env::var(TIMEOUT_VARIABLE).ok())).await;
        drop(marker);
        result
    }
}

thread_local! {
    static MARKING: Cell<bool> = const { Cell::new(false) };
}

/// While alive, no other marking starts.
struct MarkingGuard;

impl Drop for MarkingGuard {
    fn drop(&mut self) {
        MARKING.with(|marking| marking.set(false));
    }
}

#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
fn begin_marking() -> Result<MarkingGuard, AlreadyMarking> {
    if MARKING.with(|marking| marking.replace(true)) {
        Err(AlreadyMarking)
    } else {
        Ok(MarkingGuard)
    }
}

/// The timeout in `value`, a number of seconds, if it is one.
#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
fn timeout_from(value: Option<String>) -> Option<Duration> {
    let seconds = value?.trim().parse::<f64>().ok()?;
    (seconds.is_finite() && seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// `result`, or `Cancelled` once `timeout` passes.
#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
async fn with_timeout<T>(
    result: impl Future<Output = anyhow::Result<T>>,
    timeout: Option<Duration>,
) -> anyhow::Result<T> {
    let Some(timeout) = timeout else {
        return result.await;
    };

    match future::select(pin!(result), pin!(glib::timeout_future(timeout))).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => {
            tracing::warn!(?timeout, "Area marking timed out; cancelling it");
            Err(Cancelled::new("area marking").into())
        }
    }
}

/// The layer to preload with `initial`, and the rectangle: on its monitor,
/// if connected and the rectangle fits on it, even with another scale.
#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
fn preload(initial: &MarkedArea, monitors: &[MonitorInfo]) -> Option<(usize, Rect)> {
    let index = monitors
        .iter()
        .position(|monitor| monitor.connector == initial.connector)?;
    let (width, height) = monitor::physical_size(&monitors[index]);
    let fits = initial.x >= 0
        && initial.y >= 0
        && initial.x + initial.width <= width
        && initial.y + initial.height <= height;
    fits.then_some((
        index,
        Rect {
            x: initial.x,
            y: initial.y,
            width: initial.width,
            height: initial.height,
        },
    ))
}

/// The marked area of `rect` on `monitor`.
#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
fn finish(rect: Rect, monitor: &MonitorInfo) -> MarkedArea {
    let (monitor_width, monitor_height) = monitor::physical_size(monitor);
    MarkedArea {
        connector: monitor.connector.clone(),
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        monitor_width,
        monitor_height,
        scale: monitor.scale,
    }
}

/// Whether each layer has the rectangle and the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct LayerFocus {
    has_rect: bool,
    has_pointer: bool,
}

/// The layer that keys act on and that holds the keyboard: the one with
/// the rectangle, else the one with the pointer, else the first.
#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
fn active_layer(layers: &[LayerFocus]) -> Option<usize> {
    layers
        .iter()
        .position(|layer| layer.has_rect)
        .or_else(|| layers.iter().position(|layer| layer.has_pointer))
        .or_else(|| (!layers.is_empty()).then_some(0))
}

/// What a key does while marking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    Cancel,
    Confirm,
    Nudge(i32, i32),
}

/// Escape cancels, Enter confirms, the arrows move by 1 pixel, by 10 with
/// Shift.
#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]
fn key_action(key: gdk::Key, shift: bool) -> Option<KeyAction> {
    let step = if shift { 10 } else { 1 };
    match key {
        gdk::Key::Escape => Some(KeyAction::Cancel),
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => Some(KeyAction::Confirm),
        gdk::Key::Left | gdk::Key::KP_Left => Some(KeyAction::Nudge(-step, 0)),
        gdk::Key::Right | gdk::Key::KP_Right => Some(KeyAction::Nudge(step, 0)),
        gdk::Key::Up | gdk::Key::KP_Up => Some(KeyAction::Nudge(0, -step)),
        gdk::Key::Down | gdk::Key::KP_Down => Some(KeyAction::Nudge(0, step)),
        _ => None,
    }
}

#[cfg(feature = "layer-shell")]
use self::layers::AreaMarker;

#[cfg(feature = "layer-shell")]
mod layers {
    use std::{cell::RefCell, rc::Rc};

    use futures_channel::oneshot;
    use gtk::{gdk, glib, prelude::*};

    use super::{
        KeyAction, LayerFocus, active_layer, finish, key_action, preload,
        view::{Events, LayerEvent, MarkerLayer},
    };
    use crate::{
        areas::{MarkedArea, MonitorInfo},
        cancelled::Cancelled,
    };

    type Sender = oneshot::Sender<anyhow::Result<MarkedArea>>;

    /// The layers of one marking. Dropping it destroys them.
    pub struct AreaMarker {
        inner: Rc<Inner>,
    }

    struct Inner {
        layers: RefCell<Vec<Entry>>,
        result: RefCell<Option<Sender>>,
    }

    struct Entry {
        id: usize,
        monitor: gdk::Monitor,
        layer: MarkerLayer,
        invalidate_handler_id: Option<glib::SignalHandlerId>,
    }

    impl AreaMarker {
        /// Builds a layer per monitor, without showing them. The receiver
        /// gets the area or the error once.
        pub fn new(
            monitors: Vec<(gdk::Monitor, MonitorInfo)>,
            initial: Option<&MarkedArea>,
        ) -> (
            Self,
            impl Future<Output = anyhow::Result<MarkedArea>> + use<>,
        ) {
            let infos = monitors
                .iter()
                .map(|(_, info)| info.clone())
                .collect::<Vec<_>>();
            let preloaded = initial.and_then(|initial| preload(initial, &infos));

            let (sender, receiver) = oneshot::channel();
            let inner = Rc::new(Inner {
                layers: RefCell::new(Vec::new()),
                result: RefCell::new(Some(sender)),
            });

            let entries = monitors
                .into_iter()
                .enumerate()
                .map(|(id, (monitor, info))| {
                    let weak = Rc::downgrade(&inner);
                    let events: Events = Rc::new(move |event| {
                        weak.upgrade().is_some_and(|inner| inner.handle(id, event))
                    });
                    let initial = preloaded
                        .filter(|(index, _)| *index == id)
                        .map(|(_, rect)| rect);
                    let layer = MarkerLayer::new(&monitor, info, initial, events);

                    let weak = Rc::downgrade(&inner);
                    let invalidate_handler_id = monitor.connect_invalidate(move |_| {
                        if let Some(inner) = weak.upgrade() {
                            inner.monitor_gone(id);
                        }
                    });

                    Entry {
                        id,
                        monitor,
                        layer,
                        invalidate_handler_id: Some(invalidate_handler_id),
                    }
                })
                .collect();
            inner.layers.replace(entries);

            // One layer at a time holds the keyboard: the protocol has no
            // way to ask for the focus.
            if let Some(layer) = inner.active_layer() {
                inner.give_keyboard(layer.0);
            }

            let result = async move {
                receiver
                    .await
                    .unwrap_or_else(|_| Err(Cancelled::new("area marking").into()))
            };
            (Self { inner }, result)
        }

        pub fn present(&self) {
            for entry in self.inner.layers.borrow().iter() {
                entry.layer.present();
            }
        }

        #[cfg(test)]
        fn handle(&self, id: usize, event: LayerEvent) -> bool {
            self.inner.handle(id, event)
        }

        #[cfg(test)]
        fn layer(&self, id: usize) -> Option<MarkerLayer> {
            self.inner
                .layers
                .borrow()
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.layer.clone())
        }
    }

    impl Drop for AreaMarker {
        fn drop(&mut self) {
            for mut entry in self.inner.layers.take() {
                entry.destroy();
            }
        }
    }

    impl Entry {
        fn destroy(&mut self) {
            if let Some(handler_id) = self.invalidate_handler_id.take() {
                self.monitor.disconnect(handler_id);
            }
            self.layer.destroy();
        }
    }

    impl Inner {
        /// Acts on an event of the layer `id`. Keys act on the active layer,
        /// whichever layer has the keyboard.
        fn handle(&self, id: usize, event: LayerEvent) -> bool {
            if self.result.borrow().is_none() {
                // Already answered; the layers go away soon.
                return true;
            }

            match event {
                LayerEvent::Pressed => {
                    // The area lives on a single monitor.
                    for entry in self.layers.borrow().iter() {
                        if entry.id != id {
                            entry.layer.clear();
                        }
                    }
                    self.give_keyboard(id);
                    false
                }
                LayerEvent::PointerEntered => {
                    if !self
                        .layers
                        .borrow()
                        .iter()
                        .any(|entry| entry.layer.rect().is_some())
                    {
                        self.give_keyboard(id);
                    }
                    false
                }
                LayerEvent::Cancel => {
                    self.cancel();
                    true
                }
                LayerEvent::Confirm => self.confirm(Some(id)),
                LayerEvent::Key { key, shift } => match key_action(key, shift) {
                    Some(KeyAction::Cancel) => {
                        self.cancel();
                        true
                    }
                    Some(KeyAction::Confirm) => self.confirm(None),
                    Some(KeyAction::Nudge(dx, dy)) => {
                        let Some((_, layer)) = self.active_layer() else {
                            return false;
                        };
                        layer.nudge(dx, dy);
                        true
                    }
                    None => false,
                },
            }
        }

        /// The id and the layer that keys act on.
        fn active_layer(&self) -> Option<(usize, MarkerLayer)> {
            let layers = self.layers.borrow();
            let focus = layers
                .iter()
                .map(|entry| LayerFocus {
                    has_rect: entry.layer.rect().is_some(),
                    has_pointer: entry.layer.has_pointer(),
                })
                .collect::<Vec<_>>();
            let entry = &layers[active_layer(&focus)?];
            Some((entry.id, entry.layer.clone()))
        }

        /// Answers with the area of the layer `id`, or of the active one.
        fn confirm(&self, id: Option<usize>) -> bool {
            let layer = match id {
                Some(id) => self
                    .layers
                    .borrow()
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| entry.layer.clone()),
                None => self.active_layer().map(|(_, layer)| layer),
            };
            let Some(area) =
                layer.and_then(|layer| layer.rect().map(|rect| finish(rect, layer.info())))
            else {
                return false;
            };
            self.answer(Ok(area));
            true
        }

        fn cancel(&self) {
            self.answer(Err(Cancelled::new("area marking").into()));
        }

        fn answer(&self, result: anyhow::Result<MarkedArea>) {
            if let Some(sender) = self.result.take() {
                let _ = sender.send(result);
            }
        }

        /// The layer `id` takes the keyboard, Exclusive; the others give it
        /// up.
        fn give_keyboard(&self, id: usize) {
            for entry in self.layers.borrow().iter() {
                entry.layer.set_keyboard(entry.id == id);
            }
        }

        /// The monitor of the layer `id` was disconnected: its layer goes,
        /// with the rectangle if it had it. Without layers, the marking is
        /// cancelled.
        fn monitor_gone(&self, id: usize) {
            let removed = {
                let mut layers = self.layers.borrow_mut();
                let (removed, kept) = layers
                    .drain(..)
                    .partition::<Vec<_>, _>(|entry| entry.id == id);
                *layers = kept;
                removed
            };
            for mut entry in removed {
                entry.destroy();
            }
            if self.layers.borrow().is_empty() {
                self.cancel();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::area_marker::{geometry::Rect, monitor};

        /// Two layers on the first monitor, as if they were two monitors:
        /// built, never presented.
        fn two_layers(
            initial: Option<&MarkedArea>,
        ) -> Option<(
            AreaMarker,
            impl Future<Output = anyhow::Result<MarkedArea>> + use<>,
            gdk::Monitor,
        )> {
            if crate::overlay::availability().is_err() {
                eprintln!("Skipped: layers unavailable");
                return None;
            }
            let Some((gdk_monitor, info)) = gdk::Display::default()
                .and_then(|display| monitor::current(&display).into_iter().next())
            else {
                eprintln!("Skipped: no monitor");
                return None;
            };
            let left = MonitorInfo {
                connector: "LEFT-1".into(),
                ..info.clone()
            };
            let right = MonitorInfo {
                connector: "RIGHT-1".into(),
                ..info
            };
            let (marker, result) = AreaMarker::new(
                vec![(gdk_monitor.clone(), left), (gdk_monitor.clone(), right)],
                initial,
            );
            Some((marker, result, gdk_monitor))
        }

        fn area_on_right() -> MarkedArea {
            MarkedArea {
                connector: "RIGHT-1".into(),
                x: 10,
                y: 20,
                width: 400,
                height: 300,
                monitor_width: 0,
                monitor_height: 0,
                scale: 1.0,
            }
        }

        fn rect_on_right() -> Rect {
            Rect {
                x: 10,
                y: 20,
                width: 400,
                height: 300,
            }
        }

        fn toplevels() -> usize {
            gtk::Window::list_toplevels().len()
        }

        #[gtk::test]
        fn initial_area_is_preloaded_on_its_layer() {
            let Some((marker, _result, _)) = two_layers(Some(&area_on_right())) else {
                return;
            };
            assert_eq!(marker.layer(0).unwrap().rect(), None);
            assert_eq!(marker.layer(1).unwrap().rect(), Some(rect_on_right()));
        }

        #[gtk::test]
        fn one_layer_holds_the_keyboard_at_a_time() {
            let Some((marker, _result, _)) = two_layers(Some(&area_on_right())) else {
                return;
            };
            let keyboard = |id| marker.layer(id).unwrap().has_keyboard();
            // The layer of the initial rectangle starts with it, and keeps
            // it when the pointer enters another layer.
            assert!(!keyboard(0) && keyboard(1));
            marker.handle(0, LayerEvent::PointerEntered);
            assert!(!keyboard(0) && keyboard(1));

            // A press takes it.
            marker.handle(0, LayerEvent::Pressed);
            assert!(keyboard(0) && !keyboard(1));

            // With no rectangle anywhere, entering a layer takes it.
            marker.handle(1, LayerEvent::PointerEntered);
            assert!(!keyboard(0) && keyboard(1));
        }

        #[gtk::test]
        fn without_initial_area_the_first_layer_holds_the_keyboard() {
            let Some((marker, _result, _)) = two_layers(None) else {
                return;
            };
            assert!(marker.layer(0).unwrap().has_keyboard());
            assert!(!marker.layer(1).unwrap().has_keyboard());
        }

        #[gtk::test]
        fn press_on_a_layer_clears_the_others() {
            let Some((marker, _result, _)) = two_layers(Some(&area_on_right())) else {
                return;
            };
            marker.handle(0, LayerEvent::Pressed);
            assert_eq!(marker.layer(1).unwrap().rect(), None);
        }

        #[gtk::test]
        fn enter_returns_the_area_of_the_layer_with_the_rect() {
            let Some((marker, result, _)) = two_layers(Some(&area_on_right())) else {
                return;
            };
            let right = marker.layer(1).unwrap().info().clone();

            // The key reaches the layer without the rectangle.
            assert!(marker.handle(
                0,
                LayerEvent::Key {
                    key: gdk::Key::Return,
                    shift: false
                }
            ));
            let area = glib::MainContext::default().block_on(result).unwrap();
            assert_eq!(area, finish(rect_on_right(), &right));
        }

        #[gtk::test]
        fn arrows_move_the_rect_of_the_active_layer() {
            let Some((marker, _result, _)) = two_layers(Some(&area_on_right())) else {
                return;
            };
            assert!(marker.handle(
                0,
                LayerEvent::Key {
                    key: gdk::Key::Right,
                    shift: true
                }
            ));
            assert_eq!(marker.layer(1).unwrap().rect().unwrap().x, 20);
        }

        #[gtk::test]
        fn escape_and_the_secondary_button_cancel() {
            let Some((marker, result, _)) = two_layers(None) else {
                return;
            };
            assert!(marker.handle(
                1,
                LayerEvent::Key {
                    key: gdk::Key::Escape,
                    shift: false
                }
            ));
            let err = glib::MainContext::default().block_on(result).unwrap_err();
            assert!(err.is::<Cancelled>());

            let Some((marker, result, _)) = two_layers(None) else {
                return;
            };
            assert!(marker.handle(0, LayerEvent::Cancel));
            let err = glib::MainContext::default().block_on(result).unwrap_err();
            assert!(err.is::<Cancelled>());
        }

        #[gtk::test]
        fn enter_without_a_rect_does_nothing() {
            let Some((marker, _result, _)) = two_layers(None) else {
                return;
            };
            assert!(!marker.handle(
                0,
                LayerEvent::Key {
                    key: gdk::Key::Return,
                    shift: false
                }
            ));
            assert!(marker.inner.result.borrow().is_some());
        }

        #[gtk::test]
        fn dropping_the_marker_destroys_its_layers() {
            let before = toplevels();
            let Some((marker, _result, _)) = two_layers(None) else {
                return;
            };
            assert_eq!(toplevels(), before + 2);
            drop(marker);
            assert_eq!(toplevels(), before);
        }

        #[gtk::test]
        fn a_disconnected_monitor_takes_its_layers_and_cancels() {
            let before = toplevels();
            let Some((marker, result, gdk_monitor)) = two_layers(Some(&area_on_right())) else {
                return;
            };
            gdk_monitor.emit_by_name::<()>("invalidate", &[]);
            assert_eq!(toplevels(), before);
            assert!(marker.layer(1).is_none());

            // Nothing left to mark on.
            let err = glib::MainContext::default().block_on(result).unwrap_err();
            assert!(err.is::<Cancelled>());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::area_marker::monitor::tests::{dp3, edp};

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn area(connector: &str, rect: Rect, monitor: (i32, i32), scale: f64) -> MarkedArea {
        MarkedArea {
            connector: connector.into(),
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            monitor_width: monitor.0,
            monitor_height: monitor.1,
            scale,
        }
    }

    #[test]
    fn finish_records_the_monitor_in_physical_pixels() {
        assert_eq!(
            finish(rect(100, 80, 1280, 720), &dp3()),
            area("DP-3", rect(100, 80, 1280, 720), (1680, 1050), 1.05)
        );
        assert_eq!(
            finish(rect(0, 0, 2560, 1600), &edp()),
            area("eDP-1", rect(0, 0, 2560, 1600), (2560, 1600), 1.75)
        );
    }

    #[test]
    fn preload_on_the_connected_monitor_where_it_fits() {
        let monitors = [edp(), dp3()];
        let initial = area("DP-3", rect(100, 80, 1280, 720), (1680, 1050), 1.05);
        assert_eq!(
            preload(&initial, &monitors),
            Some((1, rect(100, 80, 1280, 720)))
        );

        // Marked at another scale: out of date, but it still fits.
        let initial = area("DP-3", rect(100, 80, 1280, 720), (1600, 1000), 1.0);
        assert_eq!(
            preload(&initial, &monitors),
            Some((1, rect(100, 80, 1280, 720)))
        );
    }

    #[test]
    fn no_preload_when_disconnected_or_too_big() {
        let monitors = [edp(), dp3()];
        let initial = area("HDMI-1", rect(0, 0, 640, 480), (1920, 1080), 1.0);
        assert_eq!(preload(&initial, &monitors), None);

        let initial = area("DP-3", rect(500, 0, 1280, 720), (1920, 1200), 1.0);
        assert_eq!(preload(&initial, &monitors), None);
    }

    #[test]
    fn keys_while_marking() {
        assert_eq!(key_action(gdk::Key::Escape, false), Some(KeyAction::Cancel));
        assert_eq!(
            key_action(gdk::Key::Return, false),
            Some(KeyAction::Confirm)
        );
        assert_eq!(
            key_action(gdk::Key::KP_Enter, true),
            Some(KeyAction::Confirm)
        );
        assert_eq!(
            key_action(gdk::Key::Left, false),
            Some(KeyAction::Nudge(-1, 0))
        );
        assert_eq!(
            key_action(gdk::Key::Down, true),
            Some(KeyAction::Nudge(0, 10))
        );
        assert_eq!(key_action(gdk::Key::a, false), None);
    }

    #[test]
    fn active_layer_has_the_rect_then_the_pointer_then_comes_first() {
        let none = LayerFocus::default();
        let rect = LayerFocus {
            has_rect: true,
            has_pointer: false,
        };
        let pointer = LayerFocus {
            has_rect: false,
            has_pointer: true,
        };

        assert_eq!(active_layer(&[]), None);
        assert_eq!(active_layer(&[none, none]), Some(0));
        assert_eq!(active_layer(&[none, pointer]), Some(1));
        assert_eq!(active_layer(&[pointer, none, rect]), Some(2));
    }

    #[test]
    fn only_one_marking_at_a_time() {
        let first = begin_marking().unwrap();
        assert!(begin_marking().is_err());
        drop(first);
        assert!(begin_marking().is_ok());
    }

    #[test]
    fn timeout_from_the_variable() {
        assert_eq!(timeout_from(None), None);
        assert_eq!(timeout_from(Some("5".into())), Some(Duration::from_secs(5)));
        assert_eq!(
            timeout_from(Some(" 0.5 ".into())),
            Some(Duration::from_millis(500))
        );
        assert_eq!(timeout_from(Some("soon".into())), None);
        assert_eq!(timeout_from(Some("0".into())), None);
        assert_eq!(timeout_from(Some("-3".into())), None);
    }

    #[test]
    fn marking_cancels_itself_after_the_timeout() {
        let started = Instant::now();
        let result = glib::MainContext::new().block_on(with_timeout(
            future::pending::<anyhow::Result<()>>(),
            Some(Duration::from_millis(200)),
        ));
        let elapsed = started.elapsed();

        assert!(result.unwrap_err().is::<Cancelled>());
        assert!(
            elapsed >= Duration::from_millis(200) && elapsed < Duration::from_millis(1000),
            "took {elapsed:?}"
        );
    }

    #[test]
    fn result_before_the_timeout_wins() {
        let started = Instant::now();
        let result = glib::MainContext::new().block_on(with_timeout(
            future::ready(anyhow::Ok(7)),
            Some(Duration::from_secs(5)),
        ));
        assert_eq!(result.unwrap(), 7);
        assert!(started.elapsed() < Duration::from_millis(500));

        let result =
            glib::MainContext::new().block_on(with_timeout(future::ready(anyhow::Ok(8)), None));
        assert_eq!(result.unwrap(), 8);
    }

    #[cfg(not(feature = "layer-shell"))]
    #[test]
    fn without_layers_marking_fails() {
        let err = glib::MainContext::new().block_on(mark(None)).unwrap_err();
        assert_eq!(
            err.downcast_ref::<LayersUnavailable>(),
            Some(&LayersUnavailable(OverlayUnavailable::NotCompiled))
        );
    }
}
