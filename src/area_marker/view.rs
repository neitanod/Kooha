//! The marking layer of one monitor: the veil, the rectangle with its
//! handles, the size bubble and the help text, and the gestures on them.

use std::{
    cell::{Cell, OnceCell, RefCell},
    rc::{Rc, Weak},
};

use gettextrs::gettext;
use gtk::{cairo, gdk, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, LayerShell};

use super::{
    geometry::{Handle, MarkState, Rect, parse_size},
    monitor,
};
use crate::{areas::MonitorInfo, overlay};

const LAYER_NAMESPACE: &str = "kooha-area-marker";

/// Side of the square handles, and how far from them a press still grabs
/// them, in logical pixels.
const HANDLE_SIZE: f64 = 8.0;
/// Space between the rectangle and the size bubble, in logical pixels.
const BUBBLE_GAP: f64 = 8.0;
const VEIL_ALPHA: f64 = 0.30;
/// `#3584e4`, the GNOME blue.
const HANDLE_BORDER: (f64, f64, f64) = (
    0x35 as f64 / 255.0,
    0x84 as f64 / 255.0,
    0xe4 as f64 / 255.0,
);

const CSS: &str = "
window.kooha-area-marker {
  background: none;
}
window.kooha-area-marker .size-bubble,
window.kooha-area-marker .help {
  background-color: rgba(0, 0, 0, 0.8);
  color: white;
  border-radius: 6px;
  padding: 4px 8px;
}
";

/// What a layer tells the marker. The marker answers whether it used it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerEvent {
    /// A press of the primary button anywhere on the layer.
    Pressed,
    /// The pointer came into the layer.
    PointerEntered,
    /// A key the layer did not use itself (Escape, Enter, the arrows).
    Key { key: gdk::Key, shift: bool },
    /// The secondary button.
    Cancel,
    /// Enter on the size field with the size it shows.
    Confirm,
}

pub type Events = Rc<dyn Fn(LayerEvent) -> bool>;

/// The marking layer over one whole monitor. `destroy()` takes it off the
/// screen.
#[derive(Clone)]
pub struct MarkerLayer(Rc<Inner>);

struct Inner {
    window: gtk::Window,
    drawing_area: gtk::DrawingArea,
    bubble: gtk::Box,
    label: gtk::Label,
    entry: gtk::Entry,
    help: gtk::Label,
    info: MonitorInfo,
    state: RefCell<MarkState>,
    has_pointer: Cell<bool>,
    events: Events,
}

impl MarkerLayer {
    /// Only callable when `overlay::availability()` is Ok. `initial` is in
    /// physical pixels of the monitor.
    pub fn new(
        monitor: &gdk::Monitor,
        info: MonitorInfo,
        initial: Option<Rect>,
        events: Events,
    ) -> Self {
        load_css(&monitor.display());

        let drawing_area = gtk::DrawingArea::builder()
            .focusable(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        let label = gtk::Label::new(None);
        let entry = gtk::Entry::builder()
            .width_chars(10)
            .max_width_chars(10)
            .tooltip_text(gettext("Size"))
            .build();
        let bubble = gtk::Box::builder()
            .css_classes(["size-bubble"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .visible(false)
            .build();
        bubble.append(&label);
        bubble.append(&entry);
        let help = gtk::Label::builder()
            .label(gettext("Drag to mark · Enter confirms · Esc cancels"))
            .css_classes(["help"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(24)
            .can_target(false)
            .visible(false)
            .build();

        let overlay_widget = gtk::Overlay::builder().child(&drawing_area).build();
        overlay_widget.add_overlay(&bubble);
        overlay_widget.add_overlay(&help);

        // Never a window of the app: `Application::window()` assumes that
        // every app window is a `Window`.
        let window = gtk::Window::builder()
            .decorated(false)
            .css_classes([LAYER_NAMESPACE])
            .child(&overlay_widget)
            .build();
        overlay::init_overlay(&window, monitor, LAYER_NAMESPACE);
        overlay::anchor_all_edges(&window);
        // The marker gives the keyboard to one layer.
        window.set_keyboard_mode(KeyboardMode::None);

        let bounds = monitor::physical_size(&info);
        let inner = Rc::new(Inner {
            window,
            drawing_area,
            bubble,
            label,
            entry,
            help,
            info,
            state: RefCell::new(MarkState::new(bounds, initial)),
            has_pointer: Cell::new(false),
            events,
        });
        Inner::connect(&inner);
        inner.update(true);

        Self(inner)
    }

    #[cfg(test)]
    pub fn state(&self) -> MarkState {
        self.0.state.borrow().clone()
    }

    pub fn rect(&self) -> Option<Rect> {
        self.0.state.borrow().rect()
    }

    pub fn info(&self) -> &MonitorInfo {
        &self.0.info
    }

    pub fn has_pointer(&self) -> bool {
        self.0.has_pointer.get()
    }

    /// Forgets the rectangle: the area went to another monitor.
    pub fn clear(&self) {
        self.0.state.borrow_mut().clear();
        self.0.update(true);
    }

    pub fn nudge(&self, dx: i32, dy: i32) {
        self.0.state.borrow_mut().nudge(dx, dy);
        self.0.update(false);
    }

    /// Takes the keyboard (Exclusive) or gives it up (None). The protocol
    /// has no way to ask for the focus, so only one layer at a time takes
    /// it.
    pub fn set_keyboard(&self, exclusive: bool) {
        self.0.window.set_keyboard_mode(if exclusive {
            KeyboardMode::Exclusive
        } else {
            KeyboardMode::None
        });
    }

    #[cfg(test)]
    pub fn has_keyboard(&self) -> bool {
        self.0.window.keyboard_mode() == KeyboardMode::Exclusive
    }

    pub fn present(&self) {
        self.0.window.present();
    }

    pub fn destroy(&self) {
        self.0.window.destroy();
    }
}

impl Inner {
    fn connect(this: &Rc<Self>) {
        let weak = Rc::downgrade(this);
        this.drawing_area.set_draw_func(glib::clone!(
            #[strong]
            weak,
            move |_, cr, width, height| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let state = this.state.borrow();
                let rect = state.rect().map(|rect| this.logical_rect(rect));
                let handles = !state.is_dragging();
                drop(state);
                if let Err(err) =
                    draw_marker(cr, f64::from(width), f64::from(height), rect, handles)
                {
                    tracing::warn!("Failed to draw the area marker: {:?}", err);
                }
            }
        ));

        // Any press on the layer, the size field included, goes to the
        // marker first: it gives this layer the keyboard and clears the
        // others. The secondary button cancels.
        let click = gtk::GestureClick::builder()
            .button(0)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        click.connect_pressed(glib::clone!(
            #[strong]
            weak,
            move |gesture, _, _, _| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                match gesture.current_button() {
                    gdk::BUTTON_PRIMARY => {
                        (this.events)(LayerEvent::Pressed);
                    }
                    gdk::BUTTON_SECONDARY => {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        (this.events)(LayerEvent::Cancel);
                    }
                    _ => {}
                }
            }
        ));
        this.window.add_controller(click);

        let drag = gtk::GestureDrag::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        drag.connect_drag_begin(with(&weak, |this, _: &gtk::GestureDrag, x, y| {
            this.drawing_area.grab_focus();
            let (x, y) = monitor::to_physical(&this.info, x, y);
            this.state.borrow_mut().press(x, y, this.slop());
            this.update(false);
        }));
        drag.connect_drag_update(with(&weak, |this, gesture: &gtk::GestureDrag, dx, dy| {
            let Some((x, y)) = gesture.start_point() else {
                return;
            };
            let (x, y) = monitor::to_physical(&this.info, x + dx, y + dy);
            this.state.borrow_mut().motion(x, y);
            this.update(false);
        }));
        drag.connect_drag_end(with(&weak, |this, _: &gtk::GestureDrag, _, _| {
            this.state.borrow_mut().release();
            this.update(true);
        }));
        this.drawing_area.add_controller(drag);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(with(&weak, |this, _: &gtk::EventControllerMotion, x, y| {
            let (x, y) = monitor::to_physical(&this.info, x, y);
            let state = this.state.borrow();
            // The cursor of the gesture in course stays.
            if state.is_dragging() {
                return;
            }
            let handle = state.hit(x, y, this.slop());
            drop(state);
            this.drawing_area
                .set_cursor_from_name(Some(cursor_name(handle)));
        }));
        this.drawing_area.add_controller(motion);

        let pointer = gtk::EventControllerMotion::new();
        pointer.connect_enter(with(&weak, |this, _: &gtk::EventControllerMotion, _, _| {
            this.has_pointer.set(true);
            this.help.set_visible(true);
            (this.events)(LayerEvent::PointerEntered);
        }));
        pointer.connect_leave(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    this.has_pointer.set(false);
                    this.help.set_visible(false);
                }
            }
        ));
        this.window.add_controller(pointer);

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[strong]
            weak,
            move |_, key, _, modifiers| {
                let Some(this) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                if (this.events)(LayerEvent::Key { key, shift }) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        this.window.add_controller(keys);

        this.entry.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |entry| {
                if let Some(this) = weak.upgrade() {
                    this.size_entered(entry.text().as_str());
                }
            }
        ));
        this.entry
            .connect_changed(|entry| entry.remove_css_class("error"));
    }

    /// Enter on the size field: a new size is applied; the size already
    /// shown confirms.
    fn size_entered(&self, text: &str) {
        let current = self
            .state
            .borrow()
            .rect()
            .map(|rect| (rect.width, rect.height));
        let Some((width, height)) = parse_size(text) else {
            self.entry.add_css_class("error");
            return;
        };
        if current == Some((width, height)) {
            (self.events)(LayerEvent::Confirm);
            return;
        }

        let result = self.state.borrow_mut().set_size(width, height);
        match result {
            Ok(()) => self.update(true),
            Err(_) => self.entry.add_css_class("error"),
        }
    }

    /// Redraws, and places the size bubble next to the rectangle. With
    /// `sync_entry`, the size field shows the size again.
    fn update(&self, sync_entry: bool) {
        self.drawing_area.queue_draw();

        let state = self.state.borrow();
        let Some(rect) = state.rect() else {
            self.bubble.set_visible(false);
            return;
        };
        let dragging = state.is_dragging();
        drop(state);

        // While dragging, a label; once released, the same size, editable.
        let size = format!("{} × {}", rect.width, rect.height);
        self.label.set_label(&size);
        self.label.set_visible(dragging);
        self.entry.set_visible(!dragging);
        if sync_entry {
            self.entry
                .set_text(&format!("{}×{}", rect.width, rect.height));
        }
        self.bubble.set_visible(true);

        let (_, natural) = self.bubble.preferred_size();
        let (_, _, area_width, area_height) = self.info.logical;
        let (x, y) = bubble_position(
            self.logical_rect(rect),
            (f64::from(natural.width()), f64::from(natural.height())),
            (f64::from(area_width), f64::from(area_height)),
        );
        self.bubble.set_margin_start(x.round() as i32);
        self.bubble.set_margin_top(y.round() as i32);
    }

    /// `rect` as (x, y, width, height) in logical pixels of the layer.
    fn logical_rect(&self, rect: Rect) -> (f64, f64, f64, f64) {
        let (x, y) = monitor::to_logical(&self.info, rect.x, rect.y);
        let (width, height) = monitor::to_logical(&self.info, rect.width, rect.height);
        (x, y, width, height)
    }

    /// How far from a handle a press still grabs it, in physical pixels.
    fn slop(&self) -> i32 {
        (HANDLE_SIZE * self.info.scale).round() as i32
    }
}

/// A signal handler that runs `f` while the layer is alive.
fn with<W: 'static, A: 'static, B: 'static>(
    weak: &Weak<Inner>,
    f: impl Fn(&Inner, &W, A, B) + 'static,
) -> impl Fn(&W, A, B) + 'static {
    let weak = weak.clone();
    move |widget, a, b| {
        if let Some(this) = weak.upgrade() {
            f(&this, widget, a, b);
        }
    }
}

thread_local! {
    static CSS_LOADED: OnceCell<()> = const { OnceCell::new() };
}

fn load_css(display: &gdk::Display) {
    CSS_LOADED.with(|loaded| {
        loaded.get_or_init(|| {
            let provider = gtk::CssProvider::new();
            provider.load_from_string(CSS);
            gtk::style_context_add_provider_for_display(
                display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        });
    });
}

/// The cursor that tells what a press there would do.
pub fn cursor_name(handle: Option<Handle>) -> &'static str {
    match handle {
        None => "crosshair",
        Some(Handle::Inside) => "move",
        Some(Handle::NW | Handle::SE) => "nwse-resize",
        Some(Handle::NE | Handle::SW) => "nesw-resize",
        Some(Handle::N | Handle::S) => "ns-resize",
        Some(Handle::E | Handle::W) => "ew-resize",
    }
}

/// Top-left corner of the size bubble: under the rectangle, right-aligned
/// with it; above it if it does not fit under; inside its bottom if it fits
/// in neither. Always on the layer.
pub fn bubble_position(
    rect: (f64, f64, f64, f64),
    bubble: (f64, f64),
    area: (f64, f64),
) -> (f64, f64) {
    let (x, y, width, height) = rect;
    let (bubble_width, bubble_height) = bubble;
    let (area_width, area_height) = area;

    let left = (x + width - bubble_width).clamp(0.0, (area_width - bubble_width).max(0.0));
    let under = y + height + BUBBLE_GAP;
    let above = y - BUBBLE_GAP - bubble_height;
    let top = if under + bubble_height <= area_height {
        under
    } else if above >= 0.0 {
        above
    } else {
        (y + height - BUBBLE_GAP - bubble_height).clamp(0.0, (area_height - bubble_height).max(0.0))
    };
    (left, top)
}

/// The veil over the monitor except the rectangle, its white outline, and
/// the handles once the rectangle is released. All in logical pixels.
pub fn draw_marker(
    cr: &cairo::Context,
    width: f64,
    height: f64,
    rect: Option<(f64, f64, f64, f64)>,
    handles: bool,
) -> Result<(), cairo::Error> {
    cr.set_source_rgba(0.0, 0.0, 0.0, VEIL_ALPHA);
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, width, height);
    let Some((x, y, width, height)) = rect else {
        return cr.fill();
    };
    cr.rectangle(x, y, width, height);
    cr.fill()?;

    // 1 px inside the rectangle, on whole pixels.
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(1.0);
    cr.rectangle(
        x + 0.5,
        y + 0.5,
        (width - 1.0).max(0.0),
        (height - 1.0).max(0.0),
    );
    cr.stroke()?;

    if !handles {
        return Ok(());
    }
    let half = HANDLE_SIZE / 2.0;
    for (center_x, center_y) in [
        (x, y),
        (x + width / 2.0, y),
        (x + width, y),
        (x + width, y + height / 2.0),
        (x + width, y + height),
        (x + width / 2.0, y + height),
        (x, y + height),
        (x, y + height / 2.0),
    ] {
        cr.rectangle(center_x - half, center_y - half, HANDLE_SIZE, HANDLE_SIZE);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.fill_preserve()?;
        cr.set_source_rgb(HANDLE_BORDER.0, HANDLE_BORDER.1, HANDLE_BORDER.2);
        cr.stroke()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_tells_what_a_press_does() {
        assert_eq!(cursor_name(None), "crosshair");
        assert_eq!(cursor_name(Some(Handle::Inside)), "move");
        assert_eq!(cursor_name(Some(Handle::NW)), "nwse-resize");
        assert_eq!(cursor_name(Some(Handle::SE)), "nwse-resize");
        assert_eq!(cursor_name(Some(Handle::NE)), "nesw-resize");
        assert_eq!(cursor_name(Some(Handle::SW)), "nesw-resize");
        assert_eq!(cursor_name(Some(Handle::N)), "ns-resize");
        assert_eq!(cursor_name(Some(Handle::S)), "ns-resize");
        assert_eq!(cursor_name(Some(Handle::E)), "ew-resize");
        assert_eq!(cursor_name(Some(Handle::W)), "ew-resize");
    }

    #[test]
    fn bubble_goes_under_then_above_then_inside() {
        let area = (1600.0, 1000.0);
        let bubble = (100.0, 30.0);

        // Under, right-aligned.
        assert_eq!(
            bubble_position((200.0, 100.0, 400.0, 300.0), bubble, area),
            (500.0, 400.0 + BUBBLE_GAP)
        );
        // Above, when the rectangle reaches the bottom.
        assert_eq!(
            bubble_position((200.0, 600.0, 400.0, 390.0), bubble, area),
            (500.0, 600.0 - BUBBLE_GAP - 30.0)
        );
        // Inside, when it spans the whole height; kept on the layer.
        assert_eq!(
            bubble_position((0.0, 0.0, 50.0, 1000.0), bubble, area),
            (0.0, 1000.0 - BUBBLE_GAP - 30.0)
        );
    }

    /// Pixels of a transparent 60×60 surface after `draw_marker`, as RGBA.
    fn render(rect: Option<(f64, f64, f64, f64)>, handles: bool) -> Vec<Vec<[u8; 4]>> {
        const SIZE: i32 = 60;

        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, SIZE, SIZE).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            draw_marker(&cr, f64::from(SIZE), f64::from(SIZE), rect, handles).unwrap();
        }
        surface.flush();

        let stride = surface.stride() as usize;
        let data = surface.data().unwrap();
        (0..SIZE as usize)
            .map(|y| {
                (0..SIZE as usize)
                    .map(|x| {
                        let offset = y * stride + x * 4;
                        let pixel =
                            u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap());
                        let [a, r, g, b] = pixel.to_be_bytes();
                        [r, g, b, a]
                    })
                    .collect()
            })
            .collect()
    }

    /// Cairo truncates 0.30 × 255 = 76.5.
    const VEIL_PX: [u8; 4] = [0, 0, 0, 76];

    #[test]
    fn veil_everywhere_but_the_rectangle() {
        let veil = VEIL_PX[3];

        let pixels = render(None, false);
        assert_eq!(pixels[30][30], [0, 0, 0, veil]);

        let pixels = render(Some((20.0, 20.0, 20.0, 20.0)), false);
        assert_eq!(pixels[5][5], [0, 0, 0, veil]);
        // Clear inside, white on the outline.
        assert_eq!(pixels[30][30], [0, 0, 0, 0]);
        assert_eq!(pixels[20][30], [255, 255, 255, 255]);
        assert_eq!(pixels[30][39], [255, 255, 255, 255]);
    }

    #[test]
    fn handles_only_once_released() {
        // The middle of the left side, 3 px out of the rectangle.
        let veil = VEIL_PX[3];
        assert_eq!(
            render(Some((20.0, 20.0, 20.0, 20.0)), false)[30][17],
            [0, 0, 0, veil]
        );
        assert_eq!(render(Some((20.0, 20.0, 20.0, 20.0)), true)[30][17][3], 255);
    }

    /// The layer is built, never presented.
    #[gtk::test]
    fn layer_starts_with_the_initial_rect() {
        if overlay::availability().is_err() {
            eprintln!("Skipped: layers unavailable");
            return;
        }
        let Some((gdk_monitor, info)) = gdk::Display::default()
            .and_then(|display| monitor::current(&display).into_iter().next())
        else {
            eprintln!("Skipped: no monitor");
            return;
        };

        let initial = Rect {
            x: 10,
            y: 20,
            width: 400,
            height: 300,
        };
        let layer = MarkerLayer::new(&gdk_monitor, info, Some(initial), Rc::new(|_| false));
        assert_eq!(layer.state().rect(), Some(initial));
        assert!(!layer.0.window.get_visible());
        assert!(GtkWindowExt::application(&layer.0.window).is_none());
        assert!(layer.0.bubble.get_visible());
        assert!(layer.0.entry.get_visible());
        assert_eq!(EditableExt::text(&layer.0.entry), "400×300");

        layer.clear();
        assert_eq!(layer.state().rect(), None);
        assert!(!layer.0.bubble.get_visible());
        layer.destroy();
    }
}
