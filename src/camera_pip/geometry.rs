//! Pure geometry and state of the camera `PiP`, in logical pixels.

use crate::settings::CameraShape;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipShape {
    Circle,
    RoundedRectangle,
}

pub const MIN_SIZE: i32 = 96;
pub const MAX_SIZE: i32 = 640;
pub const EDGE_MARGIN: i32 = 24;

/// How much one scroll step grows or shrinks the `PiP`.
const SCROLL_STEP_FACTOR: f64 = 1.1;

impl PipShape {
    pub fn toggled(self) -> Self {
        match self {
            Self::Circle => Self::RoundedRectangle,
            Self::RoundedRectangle => Self::Circle,
        }
    }

    /// Width for a given height; the rectangle is 16:9.
    pub fn width_for(self, height: i32) -> i32 {
        match self {
            Self::Circle => height,
            Self::RoundedRectangle => (height as f64 * 16.0 / 9.0).round() as i32,
        }
    }

    pub fn corner_radius(self, height: i32) -> f32 {
        match self {
            Self::Circle => height as f32 / 2.0,
            Self::RoundedRectangle => height as f32 * 0.12,
        }
    }
}

// The `camera-shape` key has <choices>, so gsettings-macro generates the enum
// `settings::CameraShape` (gsettings-macro 0.2.3, src/generators/string.rs).
impl From<CameraShape> for PipShape {
    fn from(shape: CameraShape) -> Self {
        match shape {
            CameraShape::Circle => Self::Circle,
            CameraShape::RoundedRectangle => Self::RoundedRectangle,
        }
    }
}

impl From<PipShape> for CameraShape {
    fn from(shape: PipShape) -> Self {
        match shape {
            PipShape::Circle => Self::Circle,
            PipShape::RoundedRectangle => Self::RoundedRectangle,
        }
    }
}

/// Scroll up grows 10 %, scroll down shrinks 10 %, clamped to `MIN_SIZE..=MAX_SIZE`.
///
/// A smooth scroll delta scales the step proportionally.
pub fn resized(size: i32, scroll_dy: f64) -> i32 {
    let factor = SCROLL_STEP_FACTOR.powf(-scroll_dy);
    let new_size = (size as f64 * factor).round() as i32;
    new_size.clamp(MIN_SIZE, MAX_SIZE)
}

/// Largest height <= `size` whose `PiP` (in `shape`) fits inside the monitor with
/// `EDGE_MARGIN` on every side; never below `MIN_SIZE` (codex r1 #7).
pub fn fitted_size(shape: PipShape, size: i32, monitor: (i32, i32)) -> i32 {
    let (available_width, available_height) =
        (monitor.0 - 2 * EDGE_MARGIN, monitor.1 - 2 * EDGE_MARGIN);

    let mut height = size.min(available_height);
    while height > MIN_SIZE && shape.width_for(height) > available_width {
        height -= 1;
    }

    height.max(MIN_SIZE)
}

/// Clamps a top-left position so the whole `PiP` stays inside the monitor.
pub fn clamp_position(pos: (i32, i32), pip: (i32, i32), monitor: (i32, i32)) -> (i32, i32) {
    let max_x = (monitor.0 - pip.0).max(0);
    let max_y = (monitor.1 - pip.1).max(0);
    (pos.0.clamp(0, max_x), pos.1.clamp(0, max_y))
}

/// Bottom-right with `EDGE_MARGIN`.
pub fn default_position(pip: (i32, i32), monitor: (i32, i32)) -> (i32, i32) {
    clamp_position(
        (
            monitor.0 - pip.0 - EDGE_MARGIN,
            monitor.1 - pip.1 - EDGE_MARGIN,
        ),
        pip,
        monitor,
    )
}

/// P13: mirrored only while no recording session is running.
pub fn should_mirror(recording_active: bool) -> bool {
    !recording_active
}

#[cfg(test)]
mod tests {
    use gtk::prelude::*;

    use super::*;
    use crate::settings::{
        Settings,
        test_utils::{settings_lock, setup_schema},
    };

    #[test]
    fn shape_toggles_both_ways() {
        assert_eq!(PipShape::Circle.toggled(), PipShape::RoundedRectangle);
        assert_eq!(PipShape::RoundedRectangle.toggled(), PipShape::Circle);
    }

    #[test]
    fn shape_converts_to_and_from_setting_enum() {
        assert_eq!(PipShape::from(CameraShape::Circle), PipShape::Circle);
        assert_eq!(
            PipShape::from(CameraShape::RoundedRectangle),
            PipShape::RoundedRectangle
        );
        assert_eq!(CameraShape::from(PipShape::Circle), CameraShape::Circle);
        assert_eq!(
            CameraShape::from(PipShape::RoundedRectangle),
            CameraShape::RoundedRectangle
        );
    }

    #[test]
    fn circle_is_square() {
        assert_eq!(PipShape::Circle.width_for(180), 180);
        assert_eq!(PipShape::Circle.corner_radius(180), 90.0);
    }

    #[test]
    fn rectangle_is_16_9() {
        assert_eq!(PipShape::RoundedRectangle.width_for(180), 320);
        assert_eq!(PipShape::RoundedRectangle.width_for(200), 356);
        assert!((PipShape::RoundedRectangle.corner_radius(200) - 24.0).abs() < 1e-4);
    }

    #[test]
    fn fitted_size_limits_wide_rectangle() {
        assert_eq!(
            fitted_size(PipShape::RoundedRectangle, 640, (1024, 768)),
            549
        );
        assert_eq!(
            fitted_size(PipShape::RoundedRectangle, 640, (1920, 1080)),
            640
        );
        assert_eq!(fitted_size(PipShape::Circle, 640, (800, 600)), 552);
        // Never below the minimum, even on a tiny monitor.
        assert_eq!(fitted_size(PipShape::Circle, 200, (100, 100)), MIN_SIZE);
    }

    #[test]
    fn persistence_roundtrip() {
        let _lock = settings_lock();
        setup_schema();

        let settings = Settings::default();
        settings.reset("camera-shape");

        assert_eq!(PipShape::from(settings.camera_shape()), PipShape::Circle);
        settings.set_camera_shape(PipShape::RoundedRectangle.into());
        assert_eq!(
            PipShape::from(settings.camera_shape()),
            PipShape::RoundedRectangle
        );

        settings.reset("camera-shape");
    }

    #[test]
    fn scroll_up_grows_ten_percent() {
        assert_eq!(resized(200, -1.0), 220);
    }

    #[test]
    fn scroll_down_shrinks() {
        assert_eq!(resized(200, 1.0), 182);
    }

    #[test]
    fn resize_clamps() {
        assert_eq!(resized(100, 1.0), MIN_SIZE);
        assert_eq!(resized(MIN_SIZE, 1.0), MIN_SIZE);
        assert_eq!(resized(600, -1.0), MAX_SIZE);
        assert_eq!(resized(MAX_SIZE, -1.0), MAX_SIZE);
    }

    #[test]
    fn clamp_keeps_inside() {
        assert_eq!(clamp_position((-10, -5), (200, 200), (1920, 1080)), (0, 0));
        assert_eq!(
            clamp_position((1900, 1000), (200, 200), (1920, 1080)),
            (1720, 880)
        );
        assert_eq!(
            clamp_position((300, 400), (200, 200), (1920, 1080)),
            (300, 400)
        );
    }

    #[test]
    fn default_is_bottom_right() {
        assert_eq!(default_position((200, 200), (1920, 1080)), (1696, 856));
    }

    #[test]
    fn mirror_only_without_recording() {
        assert!(should_mirror(false));
        assert!(!should_mirror(true));
    }
}
