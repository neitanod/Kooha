//! Marked areas: a rectangle on one monitor, in physical (video) pixels.

// The types are used by the areas list and the marker, added in later commits.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// A connected monitor, as the marker and the store see it.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// The connector name, like `DP-3`.
    pub connector: String,
    /// Logical rectangle in the global layout: (x, y, width, height).
    pub logical: (i32, i32, i32, i32),
    /// Fractional scale, like 1.75.
    pub scale: f64,
}

impl MonitorInfo {
    /// The size in physical pixels, rounded the way `area_border` matches
    /// the stream to its monitor.
    pub fn physical_size(&self) -> (i32, i32) {
        let (_, _, width, height) = self.logical;
        (self.scaled(width), self.scaled(height))
    }

    /// Converts a point in logical pixels, local to this monitor, to
    /// physical pixels.
    pub fn to_physical(&self, lx: f64, ly: f64) -> (i32, i32) {
        (
            (lx * self.scale).round() as i32,
            (ly * self.scale).round() as i32,
        )
    }

    /// Converts a point in physical pixels, local to this monitor, to
    /// logical pixels.
    pub fn to_logical(&self, px: i32, py: i32) -> (f64, f64) {
        (f64::from(px) / self.scale, f64::from(py) / self.scale)
    }

    fn scaled(&self, logical: i32) -> i32 {
        (f64::from(logical) * self.scale).round() as i32
    }
}

/// An area on one monitor, in physical pixels of that monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkedArea {
    pub connector: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Physical size and scale of the monitor when the area was marked.
    pub monitor_width: i32,
    pub monitor_height: i32,
    pub scale: f64,
}

/// A marked area with a name, in the saved list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedArea {
    pub name: String,
    #[serde(flatten)]
    pub area: MarkedArea,
}

/// Why an area can't be recorded any more.
#[derive(Debug, Clone, PartialEq)]
pub enum Staleness {
    Disconnected,
    SizeChanged { from: (i32, i32), to: (i32, i32) },
    ScaleChanged { from: f64, to: f64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edp() -> MonitorInfo {
        MonitorInfo {
            connector: "eDP-1".into(),
            logical: (457, 1080, 1463, 915),
            scale: 1.75,
        }
    }

    fn dp3() -> MonitorInfo {
        MonitorInfo {
            connector: "DP-3".into(),
            logical: (1920, 454, 1600, 1000),
            scale: 1.05,
        }
    }

    #[test]
    fn physical_size_rounds_logical_times_scale() {
        // 1463 * 1.75 = 2560.25, 915 * 1.75 = 1601.25
        assert_eq!(edp().physical_size(), (2560, 1601));
        assert_eq!(dp3().physical_size(), (1680, 1050));
    }

    #[test]
    fn to_physical_and_back_are_monitor_local() {
        let m = dp3();
        assert_eq!(m.to_physical(100.0, 80.0), (105, 84));
        let (lx, ly) = m.to_logical(105, 84);
        assert!((lx - 100.0).abs() < 0.01 && (ly - 80.0).abs() < 0.01);
    }
}
