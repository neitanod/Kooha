//! Pure geometry of the area border, in monitor-local logical pixels.

use crate::pipeline::Crop;

pub const LINE_WIDTH: f64 = 1.0; // logical px (mock-feedback.md)
pub const GAP: f64 = 2.0; // logical px between the crop and the line
pub const DASH: f64 = 4.0;
pub const GRAY: (f64, f64, f64) = (
    0xc8 as f64 / 255.0,
    0xc8 as f64 / 255.0,
    0xc8 as f64 / 255.0,
);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderGeometry {
    /// The cropped area in monitor-local logical coordinates.
    pub area: (f64, f64, f64, f64), // x, y, w, h
    /// Which sides have room outside the area: top, right, bottom, left.
    pub sides: [bool; 4],
}

impl BorderGeometry {
    /// Where the center of the line runs: left, top, right and bottom.
    ///
    /// The inner edge of the line is at least `GAP` outside the area, pushed
    /// further out to a whole pixel, so the opaque line covers whole pixels
    /// instead of blending into its neighbours.
    pub fn line_center(&self) -> (f64, f64, f64, f64) {
        let (x, y, w, h) = self.area;
        let half = LINE_WIDTH / 2.0;
        (
            (x - GAP).floor() - half,
            (y - GAP).floor() - half,
            (x + w + GAP).ceil() + half,
            (y + h + GAP).ceil() + half,
        )
    }
}

/// Only for a stream that captures a whole monitor; `monitor` is that monitor's
/// logical geometry (x, y, w, h) from `gdk::Monitor::geometry()`.
///
/// The crop is in pixels of the frame that enters `videocrop`, so each axis
/// scales on its own from that frame to the monitor's logical size.
pub fn border_geometry(crop: Crop, monitor: (i32, i32, i32, i32)) -> Option<BorderGeometry> {
    let (_, _, monitor_width, monitor_height) = monitor;
    let (frame_width, frame_height) = crop.frame;

    let sx = f64::from(monitor_width) / f64::from(frame_width);
    let sy = f64::from(monitor_height) / f64::from(frame_height);
    if !sx.is_finite() || !sy.is_finite() {
        return None;
    }

    let area = (
        f64::from(crop.left) * sx,
        f64::from(crop.top) * sy,
        f64::from(frame_width - crop.left - crop.right) * sx,
        f64::from(frame_height - crop.top - crop.bottom) * sy,
    );
    let (x, y, w, h) = area;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }

    let room = GAP + LINE_WIDTH;
    Some(BorderGeometry {
        area,
        sides: [
            y >= room,
            f64::from(monitor_width) - (x + w) >= room,
            f64::from(monitor_height) - (y + h) >= room,
            x >= room,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: usize = 0;
    const RIGHT: usize = 1;
    const BOTTOM: usize = 2;
    const LEFT: usize = 3;

    fn crop(top: i32, left: i32, right: i32, bottom: i32, frame: (i32, i32)) -> Crop {
        Crop {
            top,
            left,
            right,
            bottom,
            frame,
        }
    }

    fn assert_area_near(actual: (f64, f64, f64, f64), expected: (f64, f64, f64, f64)) {
        let pairs = [
            (actual.0, expected.0),
            (actual.1, expected.1),
            (actual.2, expected.2),
            (actual.3, expected.3),
        ];
        for (a, e) in pairs {
            assert!((a - e).abs() < 1e-6, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn scale_one_centered() {
        let border =
            border_geometry(crop(100, 200, 920, 380, (1920, 1080)), (0, 0, 1920, 1080)).unwrap();

        assert_area_near(border.area, (200.0, 100.0, 800.0, 600.0));
        assert_eq!(border.sides, [true; 4]);
    }

    #[test]
    fn hidpi_scale_two() {
        let border =
            border_geometry(crop(200, 400, 1840, 760, (3840, 2160)), (0, 0, 1920, 1080)).unwrap();

        assert_area_near(border.area, (200.0, 100.0, 800.0, 600.0));
    }

    #[test]
    fn second_monitor_offset() {
        let border = border_geometry(
            crop(100, 200, 920, 380, (1920, 1080)),
            (1920, 0, 1920, 1080),
        )
        .unwrap();

        assert_area_near(border.area, (200.0, 100.0, 800.0, 600.0));
    }

    /// `eDP-1` from `kscreen-doctor -o` (17/11/2026): 2560×1600 at scale
    /// 1.75, so the two axes scale differently.
    #[test]
    fn edp_scale_1_75() {
        let border = border_geometry(
            crop(200, 400, 560, 400, (2560, 1600)),
            (457, 1080, 1463, 915),
        )
        .unwrap();

        let sx: f64 = 1463.0 / 2560.0;
        let sy: f64 = 915.0 / 1600.0;
        assert!((sx - sy).abs() > 1e-4);
        assert_area_near(
            border.area,
            (400.0 * sx, 200.0 * sy, 1600.0 * sx, 1000.0 * sy),
        );
        assert_eq!(border.sides, [true; 4]);
    }

    /// `DP-3` from `kscreen-doctor -o` (17/11/2026): 1680×1050 at scale 1.05.
    #[test]
    fn dp3_scale_1_05() {
        let border =
            border_geometry(crop(50, 80, 100, 0, (1680, 1050)), (1920, 454, 1600, 1000)).unwrap();

        let s = 1600.0 / 1680.0;
        assert_area_near(border.area, (80.0 * s, 50.0 * s, 1500.0 * s, 1000.0 * s));
        assert_eq!(border.sides, [true, true, false, true]);
    }

    #[test]
    fn touching_left_edge_skips_left() {
        let border =
            border_geometry(crop(100, 0, 920, 380, (1920, 1080)), (0, 0, 1920, 1080)).unwrap();

        assert!(!border.sides[LEFT]);
        assert!(border.sides[TOP] && border.sides[RIGHT] && border.sides[BOTTOM]);
    }

    #[test]
    fn side_needs_room_for_gap_and_line() {
        // 2 px from the edge leave room for the gap, not for the line.
        let border = border_geometry(crop(2, 4, 4, 2, (1920, 1080)), (0, 0, 1920, 1080)).unwrap();
        assert_eq!(border.sides, [false, true, false, true]);
    }

    #[test]
    fn fractional_scale() {
        let border = border_geometry(crop(0, 300, 0, 0, (2880, 1620)), (0, 0, 1920, 1080)).unwrap();

        assert!((border.area.0 - 200.0).abs() < 1e-6);
    }

    /// A 1921×1081 stream is scaled to a 1920×1080 frame before the crop.
    #[test]
    fn odd_frame() {
        let (top, left, right, bottom) = (500, 1000, 600, 300);
        let border = border_geometry(
            crop(top, left, right, bottom, (1920, 1080)),
            (0, 0, 1921, 1081),
        )
        .unwrap();

        // With real numbers, the crop is measured on the 1921×1081 stream,
        // which is the logical monitor.
        let (x, y, w, h) = border.area;
        assert!((x - f64::from(left)).abs() <= 1.0);
        assert!((y - f64::from(top)).abs() <= 1.0);
        assert!((x + w - f64::from(1921 - right)).abs() <= 1.0);
        assert!((y + h - f64::from(1081 - bottom)).abs() <= 1.0);
    }

    #[test]
    fn empty_area_has_no_border() {
        assert_eq!(
            border_geometry(crop(0, 960, 960, 0, (1920, 1080)), (0, 0, 1920, 1080)),
            None
        );
        assert_eq!(
            border_geometry(crop(0, 0, 0, 0, (0, 1080)), (0, 0, 1920, 1080)),
            None
        );
    }

    #[test]
    fn line_stays_outside_crop() {
        for &(x, y, w, h) in &[
            (200.0, 100.0, 800.0, 600.0),
            (228.59, 114.375, 914.375, 571.875),
            (3.0, 3.0, 10.0, 10.0),
            (0.5, 7.25, 1.0, 0.75),
        ] {
            let border = BorderGeometry {
                area: (x, y, w, h),
                sides: [true; 4],
            };
            let (left, top, right, bottom) = border.line_center();
            let half = LINE_WIDTH / 2.0;

            // The inner edge of the line is at least `GAP` outside the area.
            assert!(left + half <= x - GAP, "{border:?}");
            assert!(top + half <= y - GAP, "{border:?}");
            assert!(right - half >= x + w + GAP, "{border:?}");
            assert!(bottom - half >= y + h + GAP, "{border:?}");
            // And never further than one extra pixel.
            assert!(left + half > x - GAP - 1.0, "{border:?}");
            assert!(bottom - half < y + h + GAP + 1.0, "{border:?}");
        }
    }

    #[test]
    fn line_is_pixel_aligned() {
        let border = BorderGeometry {
            area: (228.59, 114.375, 914.375, 571.875),
            sides: [true; 4],
        };
        let (left, top, right, bottom) = border.line_center();

        for edge in [left, top, right, bottom] {
            assert_eq!(edge.fract(), 0.5, "{edge}");
        }
    }
}
