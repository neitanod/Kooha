//! The layer surface that draws the area border.

use gtk::{
    cairo, gdk,
    glib::{self, clone},
    prelude::*,
};

use super::geometry::{BorderGeometry, DASH, GRAY, LINE_WIDTH};
#[cfg(feature = "layer-shell")]
use crate::overlay;

const LAYER_NAMESPACE: &str = "kooha-area-border";

/// The border on screen: a layer over the whole monitor that the mouse goes
/// through. Dropping it destroys the layer.
#[derive(Debug)]
pub struct AreaBorder {
    window: gtk::Window,
    monitor: gdk::Monitor,
    invalidate_handler_id: Option<glib::SignalHandlerId>,
}

impl AreaBorder {
    /// Only callable when `overlay::availability()` is Ok.
    pub fn new(border: BorderGeometry, monitor: &gdk::Monitor) -> Self {
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_draw_func(move |_, cr, _, _| {
            if let Err(err) = draw_border(cr, &border) {
                tracing::warn!("Failed to draw the area border: {:?}", err);
            }
        });

        // Never a window of the app: `Application::window()` assumes that
        // every app window is a `Window`.
        let window = gtk::Window::builder()
            .decorated(false)
            .css_classes(["area-border"])
            .child(&drawing_area)
            .build();
        #[cfg(feature = "layer-shell")]
        {
            overlay::init_overlay(&window, monitor, LAYER_NAMESPACE);
            overlay::anchor_all_edges(&window);
        }
        #[cfg(not(feature = "layer-shell"))]
        let _ = (monitor, LAYER_NAMESPACE);

        window.connect_realize(|window| {
            let Some(surface) = window.surface() else {
                return;
            };
            clear_input_region(&surface);
            // GTK may set its own input region when the surface is laid out
            // again, so clear it after each layout too.
            surface.connect_layout(|surface, _, _| clear_input_region(surface));
        });

        // A disconnected monitor takes the layer with it; nothing is left
        // to show the border on (fable code r1 #8).
        let invalidate_handler_id = monitor.connect_invalidate(clone!(
            #[weak]
            window,
            move |_| window.destroy()
        ));

        Self {
            window,
            monitor: monitor.clone(),
            invalidate_handler_id: Some(invalidate_handler_id),
        }
    }

    pub fn present(&self) {
        self.window.present();
    }
}

impl Drop for AreaBorder {
    fn drop(&mut self) {
        if let Some(handler_id) = self.invalidate_handler_id.take() {
            self.monitor.disconnect(handler_id);
        }
        self.window.destroy();
    }
}

/// An empty input region lets the mouse go through the whole layer.
fn clear_input_region(surface: &gdk::Surface) {
    surface.set_input_region(Some(&cairo::Region::create()));
}

/// Draws each enabled side: the whole segment in gray, then white dashes on
/// top, so the line is opaque everywhere (P15).
pub fn draw_border(cr: &cairo::Context, border: &BorderGeometry) -> Result<(), cairo::Error> {
    let (left, top, right, bottom) = border.line_center();
    let half = LINE_WIDTH / 2.0;

    // Top, right, bottom and left; each one spans the corners, so the sides
    // meet.
    let segments = [
        ((left - half, top), (right + half, top)),
        ((right, top - half), (right, bottom + half)),
        ((left - half, bottom), (right + half, bottom)),
        ((left, top - half), (left, bottom + half)),
    ];

    cr.set_line_width(LINE_WIDTH);
    cr.set_line_cap(cairo::LineCap::Butt);

    for (enabled, ((x0, y0), (x1, y1))) in border.sides.into_iter().zip(segments) {
        if !enabled {
            continue;
        }

        cr.set_dash(&[], 0.0);
        cr.set_source_rgb(GRAY.0, GRAY.1, GRAY.2);
        cr.move_to(x0, y0);
        cr.line_to(x1, y1);
        cr.stroke()?;

        cr.set_dash(&[DASH, DASH], 0.0);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.move_to(x0, y0);
        cr.line_to(x1, y1);
        cr.stroke()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE_PX: [u8; 4] = [255, 255, 255, 255];
    const GRAY_PX: [u8; 4] = [200, 200, 200, 255];
    const CLEAR_PX: [u8; 4] = [0, 0, 0, 0];

    /// Draws `border` on a transparent 60×60 surface and returns its pixels
    /// as RGBA.
    fn render(border: &BorderGeometry) -> Vec<Vec<[u8; 4]>> {
        const SIZE: i32 = 60;

        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, SIZE, SIZE).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            draw_border(&cr, border).unwrap();
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

    fn border(sides: [bool; 4]) -> BorderGeometry {
        BorderGeometry {
            area: (20.0, 20.0, 20.0, 20.0),
            sides,
        }
    }

    #[test]
    fn line_is_outside_the_area_and_the_gap() {
        let pixels = render(&border([true; 4]));

        // The area plus the 2 px gap stays untouched (criterion E1).
        for row in &pixels[18..42] {
            for pixel in &row[18..42] {
                assert_eq!(*pixel, CLEAR_PX);
            }
        }
        // The line is the first pixel outside the gap, and nothing beyond it.
        for x in 17..=42 {
            assert_eq!(pixels[17][x][3], 255, "top at x {x}");
            assert_eq!(pixels[42][x][3], 255, "bottom at x {x}");
            assert_eq!(pixels[x][17][3], 255, "left at y {x}");
            assert_eq!(pixels[x][42][3], 255, "right at y {x}");
            assert_eq!(pixels[16][x], CLEAR_PX);
            assert_eq!(pixels[43][x], CLEAR_PX);
            assert_eq!(pixels[x][16], CLEAR_PX);
            assert_eq!(pixels[x][43], CLEAR_PX);
        }
    }

    #[test]
    fn white_dashes_over_gray() {
        let pixels = render(&border([true; 4]));

        // The top line starts at x 17: 4 px white, 4 px gray, and so on.
        for x in 17..=42 {
            let expected = if (x - 17) / 4 % 2 == 0 {
                WHITE_PX
            } else {
                GRAY_PX
            };
            assert_eq!(pixels[17][x], expected, "top at x {x}");
        }
        // Every line pixel is one of the two opaque colors.
        for i in 17..=42 {
            for pixel in [pixels[17][i], pixels[42][i], pixels[i][17], pixels[i][42]] {
                assert!(pixel == WHITE_PX || pixel == GRAY_PX, "{pixel:?}");
            }
        }
    }

    /// The layer is built, never presented.
    #[gtk::test]
    fn layer_window_is_undecorated_and_appless() {
        use gtk::{gdk, glib, prelude::*};

        #[cfg(feature = "layer-shell")]
        if crate::overlay::availability().is_err() {
            eprintln!("Skipped: layers unavailable");
            return;
        }
        let Some(monitor) = gdk::Display::default()
            .and_then(|display| display.monitors().item(0))
            .and_downcast::<gdk::Monitor>()
        else {
            eprintln!("Skipped: no monitor");
            return;
        };

        let area_border = AreaBorder::new(border([true; 4]), &monitor);
        let window = area_border.window.clone();
        assert!(!window.is_decorated());
        assert!(window.application().is_none());
        assert!(window.has_css_class("area-border"));
        assert!(window.child().and_downcast::<gtk::DrawingArea>().is_some());
        assert!(!window.is_visible());

        let weak = glib::WeakRef::new();
        weak.set(Some(&window));
        drop(window);
        drop(area_border);
        assert!(weak.upgrade().is_none());
    }

    /// A border whose monitor was disconnected goes away with it (fable code
    /// r1 #8). Built, never presented.
    #[gtk::test]
    fn layer_is_destroyed_when_monitor_goes_away() {
        use gtk::{gdk, prelude::*};

        #[cfg(feature = "layer-shell")]
        if crate::overlay::availability().is_err() {
            eprintln!("Skipped: layers unavailable");
            return;
        }
        let Some(monitor) = gdk::Display::default()
            .and_then(|display| display.monitors().item(0))
            .and_downcast::<gdk::Monitor>()
        else {
            eprintln!("Skipped: no monitor");
            return;
        };

        let area_border = AreaBorder::new(border([true; 4]), &monitor);
        let window = area_border.window.clone();
        assert!(gtk::Window::list_toplevels().contains(window.upcast_ref()));

        monitor.emit_by_name::<()>("invalidate", &[]);

        assert!(!gtk::Window::list_toplevels().contains(window.upcast_ref()));
        drop(area_border);
    }

    #[test]
    fn disabled_side_is_not_drawn() {
        // The area touches the left edge of the monitor (criterion E2).
        let pixels = render(&border([true, true, true, false]));

        for row in &pixels[19..41] {
            assert_eq!(row[17], CLEAR_PX);
        }
        assert_eq!(pixels[30][42][3], 255);
    }
}
