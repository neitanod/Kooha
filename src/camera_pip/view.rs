//! The widget that draws the camera inside the `PiP` shape.

use gtk::{
    gdk, glib,
    glib::clone,
    graphene::{self, Rect},
    gsk::RoundedRect,
    prelude::*,
    subclass::prelude::*,
};

use super::geometry::PipShape;

const BORDER_WIDTH: f32 = 2.0;
const BORDER_COLOR: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.85);

mod imp {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(Debug)]
    pub struct PipView {
        pub(super) paintable: RefCell<Option<gdk::Paintable>>,
        pub(super) handler_ids: RefCell<Vec<glib::SignalHandlerId>>,
        pub(super) shape: Cell<PipShape>,
        pub(super) height: Cell<i32>,
        pub(super) mirrored: Cell<bool>,
        /// How many times the paintable asked for a redraw.
        #[cfg(test)]
        pub(super) draw_requests: Cell<u32>,
    }

    impl Default for PipView {
        fn default() -> Self {
            Self {
                paintable: RefCell::default(),
                handler_ids: RefCell::default(),
                shape: Cell::new(PipShape::Circle),
                height: Cell::new(super::super::geometry::MIN_SIZE),
                mirrored: Cell::new(false),
                #[cfg(test)]
                draw_requests: Cell::new(0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PipView {
        const NAME: &'static str = "KoohaPipView";
        type Type = super::PipView;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PipView {
        fn dispose(&self) {
            self.obj().set_paintable(None);
        }
    }

    impl WidgetImpl for PipView {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (width, height) = self.obj().size();
            let size = match orientation {
                gtk::Orientation::Horizontal => width,
                _ => height,
            };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();

            let (width, height) = obj.size();
            let (width, height) = (width as f32, height as f32);
            let bounds = Rect::new(0.0, 0.0, width, height);
            let rounded =
                RoundedRect::from_rect(bounds, self.shape.get().corner_radius(obj.height()));

            snapshot.push_rounded_clip(&rounded);
            if let Some(paintable) = self.paintable.borrow().as_ref() {
                snapshot.save();
                if self.mirrored.get() {
                    snapshot.translate(&graphene::Point::new(width, 0.0));
                    snapshot.scale(-1.0, 1.0);
                }
                let cover = cover_rect(
                    (
                        paintable.intrinsic_width() as f64,
                        paintable.intrinsic_height() as f64,
                    ),
                    (width as f64, height as f64),
                );
                snapshot.translate(&graphene::Point::new(cover.x(), cover.y()));
                paintable.snapshot(snapshot, cover.width() as f64, cover.height() as f64);
                snapshot.restore();
            } else {
                snapshot.append_color(&gdk::RGBA::BLACK, &bounds);
            }
            snapshot.pop();

            snapshot.append_border(
                &rounded,
                &[BORDER_WIDTH; 4],
                &[BORDER_COLOR, BORDER_COLOR, BORDER_COLOR, BORDER_COLOR],
            );
        }
    }
}

glib::wrapper! {
    pub struct PipView(ObjectSubclass<imp::PipView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PipView {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn height(&self) -> i32 {
        self.imp().height.get()
    }

    /// The size the view asks for, from its shape and height.
    pub fn size(&self) -> (i32, i32) {
        let height = self.height();
        (self.imp().shape.get().width_for(height), height)
    }

    pub fn set_height(&self, height: i32) {
        if self.imp().height.replace(height) != height {
            self.queue_resize();
        }
    }

    pub fn shape(&self) -> PipShape {
        self.imp().shape.get()
    }

    pub fn set_shape(&self, shape: PipShape) {
        if self.imp().shape.replace(shape) != shape {
            self.queue_resize();
        }
    }

    pub fn set_mirrored(&self, mirrored: bool) {
        if self.imp().mirrored.replace(mirrored) != mirrored {
            self.queue_draw();
        }
    }

    /// Shows `paintable`, redrawing whenever it changes (codex r1 #6).
    pub fn set_paintable(&self, paintable: Option<&gdk::Paintable>) {
        let imp = self.imp();

        if imp.paintable.borrow().as_ref() == paintable {
            return;
        }

        let previous = imp.paintable.replace(paintable.cloned());
        let mut handler_ids = imp.handler_ids.borrow_mut();
        if let Some(previous) = previous {
            for handler_id in handler_ids.drain(..) {
                previous.disconnect(handler_id);
            }
        }

        if let Some(paintable) = paintable {
            handler_ids.push(paintable.connect_invalidate_contents(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    #[cfg(test)]
                    obj.imp()
                        .draw_requests
                        .set(obj.imp().draw_requests.get() + 1);
                    obj.queue_draw();
                }
            )));
            handler_ids.push(paintable.connect_invalidate_size(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.queue_resize();
                }
            )));
        }

        self.queue_draw();
    }
}

/// Where to draw a `paintable` of that size so that it covers all of
/// `target`, centered and keeping its aspect ratio.
pub fn cover_rect(paintable: (f64, f64), target: (f64, f64)) -> graphene::Rect {
    if paintable.0 <= 0.0 || paintable.1 <= 0.0 {
        return Rect::new(0.0, 0.0, target.0 as f32, target.1 as f32);
    }

    let scale = (target.0 / paintable.0).max(target.1 / paintable.1);
    let (width, height) = (paintable.0 * scale, paintable.1 * scale);

    Rect::new(
        ((target.0 - width) / 2.0) as f32,
        ((target.1 - height) / 2.0) as f32,
        width as f32,
        height as f32,
    )
}

#[cfg(test)]
mod tests {
    use gtk::{gdk, glib, graphene, gsk};

    use super::*;

    #[gtk::test]
    fn measure_follows_shape_and_height() {
        let view = PipView::new();
        view.set_height(200);

        view.set_shape(PipShape::Circle);
        assert_eq!(view.measure(gtk::Orientation::Horizontal, -1).1, 200);
        assert_eq!(view.measure(gtk::Orientation::Vertical, -1).1, 200);

        view.set_shape(PipShape::RoundedRectangle);
        assert_eq!(view.measure(gtk::Orientation::Horizontal, -1).1, 356);
        assert_eq!(view.measure(gtk::Orientation::Vertical, -1).1, 200);
    }

    #[gtk::test]
    fn replacing_paintable_disconnects() {
        let view = PipView::new();
        let first = gtk::WidgetPaintable::new(None::<&gtk::Widget>).upcast::<gdk::Paintable>();
        let second = gtk::WidgetPaintable::new(None::<&gtk::Widget>).upcast::<gdk::Paintable>();

        view.set_paintable(Some(&first));
        first.invalidate_contents();
        assert_eq!(view.imp().draw_requests.get(), 1);

        view.set_paintable(Some(&second));
        first.invalidate_contents();
        assert_eq!(view.imp().draw_requests.get(), 1);

        second.invalidate_contents();
        assert_eq!(view.imp().draw_requests.get(), 2);
    }

    #[test]
    fn cover_fills_and_centers() {
        // Wider source: scaled by the height, cropped on the sides.
        let rect = cover_rect((2.0, 1.0), (160.0, 90.0));
        assert_eq!(
            (rect.x(), rect.y(), rect.width(), rect.height()),
            (-10.0, 0.0, 180.0, 90.0)
        );

        // Taller source: scaled by the width, cropped at the top and bottom.
        let rect = cover_rect((100.0, 100.0), (160.0, 90.0));
        assert_eq!(
            (rect.x(), rect.y(), rect.width(), rect.height()),
            (0.0, -35.0, 160.0, 160.0)
        );

        // No intrinsic size yet: fills the target.
        let rect = cover_rect((0.0, 0.0), (160.0, 90.0));
        assert_eq!(
            (rect.x(), rect.y(), rect.width(), rect.height()),
            (0.0, 0.0, 160.0, 90.0)
        );
    }

    /// Red on the left half, blue on the right half.
    fn red_blue_texture() -> gdk::Texture {
        let pixels = [
            255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
        ];
        gdk::MemoryTexture::new(
            4,
            1,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from(&pixels),
            16,
        )
        .upcast()
    }

    /// Renders `view` and returns the (red, blue) of the pixel at `(x, y)`.
    fn render_pixels(view: &PipView, points: &[(i32, i32)]) -> Vec<(u8, u8)> {
        let snapshot = gtk::Snapshot::new();
        view.imp().snapshot(&snapshot);
        let node = snapshot.to_node().expect("the view draws something");

        let (width, height) = view.size();
        let renderer = gsk::CairoRenderer::new();
        renderer.realize(None).unwrap();
        let texture = renderer.render_texture(
            &node,
            Some(&graphene::Rect::new(0.0, 0.0, width as f32, height as f32)),
        );
        renderer.unrealize();

        let stride = width as usize * 4;
        let mut data = vec![0; stride * height as usize];
        texture.download(&mut data, stride);

        // Cairo textures download as BGRA.
        points
            .iter()
            .map(|&(x, y)| {
                let offset = y as usize * stride + x as usize * 4;
                (data[offset + 2], data[offset])
            })
            .collect()
    }

    #[gtk::test]
    fn mirror_flag_flips_drawing() {
        let view = PipView::new();
        view.set_shape(PipShape::RoundedRectangle);
        view.set_height(90);
        view.set_paintable(Some(red_blue_texture().upcast_ref()));

        let points = [(40, 45), (120, 45)];

        view.set_mirrored(false);
        let [left, right] = render_pixels(&view, &points)[..] else {
            unreachable!()
        };
        assert!(left.0 > 200 && left.1 < 50, "left {left:?}");
        assert!(right.1 > 200 && right.0 < 50, "right {right:?}");

        view.set_mirrored(true);
        let [left, right] = render_pixels(&view, &points)[..] else {
            unreachable!()
        };
        assert!(left.1 > 200 && left.0 < 50, "left {left:?}");
        assert!(right.0 > 200 && right.1 < 50, "right {right:?}");
    }
}
