//! Marked areas: a rectangle on one monitor, in physical (video) pixels.

// The types are used by the areas list and the marker, added in later commits.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::i18n::gettext_f;

/// How many physical pixels a monitor size may differ by, per axis, and
/// still be the same size: logical sizes times a fractional scale round
/// differently. The same tolerance `area_border` uses.
const SIZE_TOLERANCE: i32 = 2;

/// How much a monitor scale may differ by and still be the same scale.
const SCALE_TOLERANCE: f64 = 0.01;

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

impl MarkedArea {
    /// Why this area no longer matches its monitor among `monitors`, or
    /// `None` if it can still be recorded.
    pub fn staleness(&self, monitors: &[MonitorInfo]) -> Option<Staleness> {
        let Some(monitor) = monitors.iter().find(|m| m.connector == self.connector) else {
            return Some(Staleness::Disconnected);
        };

        let from = (self.monitor_width, self.monitor_height);
        let to = monitor.physical_size();
        if (from.0 - to.0).abs() > SIZE_TOLERANCE || (from.1 - to.1).abs() > SIZE_TOLERANCE {
            return Some(Staleness::SizeChanged { from, to });
        }

        if (self.scale - monitor.scale).abs() > SCALE_TOLERANCE {
            return Some(Staleness::ScaleChanged {
                from: self.scale,
                to: monitor.scale,
            });
        }

        None
    }

    /// A label like `1280×720 on DP-3`.
    pub fn describe(&self) -> String {
        gettext_f(
            "{width}×{height} on {connector}",
            &[
                ("width", &self.width.to_string()),
                ("height", &self.height.to_string()),
                ("connector", &self.connector),
            ],
        )
    }
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

impl Staleness {
    /// Why an area on `connector` is stale, like `DP-3 is not connected`.
    pub fn describe(&self, connector: &str) -> String {
        match self {
            Self::Disconnected => {
                gettext_f("{connector} is not connected", &[("connector", connector)])
            }
            Self::SizeChanged { from, to } => gettext_f(
                "{connector} changed from {from} to {to}",
                &[
                    ("connector", connector),
                    ("from", &format!("{}×{}", from.0, from.1)),
                    ("to", &format!("{}×{}", to.0, to.1)),
                ],
            ),
            Self::ScaleChanged { from, to } => gettext_f(
                "{connector} changed scale from {from} to {to}",
                &[
                    ("connector", connector),
                    ("from", &from.to_string()),
                    ("to", &to.to_string()),
                ],
            ),
        }
    }
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

    fn area_on_dp3() -> MarkedArea {
        MarkedArea {
            connector: "DP-3".into(),
            x: 100,
            y: 80,
            width: 1280,
            height: 720,
            monitor_width: 1680,
            monitor_height: 1050,
            scale: 1.05,
        }
    }

    #[test]
    fn fresh_when_monitor_matches() {
        assert_eq!(area_on_dp3().staleness(&[edp(), dp3()]), None);
    }

    #[test]
    fn fresh_within_rounding_tolerance() {
        let mut m = dp3();
        m.logical = (1920, 454, 1602, 1001); // 1682x1051 physical
        assert_eq!(area_on_dp3().staleness(&[m]), None);
    }

    #[test]
    fn disconnected_when_connector_missing() {
        assert_eq!(
            area_on_dp3().staleness(&[edp()]),
            Some(Staleness::Disconnected)
        );
    }

    #[test]
    fn scale_changed_beyond_tolerance() {
        let mut m = dp3();
        m.scale = 1.25;
        m.logical = (1920, 454, 1344, 840); // same physical size, other scale
        assert_eq!(
            area_on_dp3().staleness(&[m]),
            Some(Staleness::ScaleChanged {
                from: 1.05,
                to: 1.25
            })
        );
    }

    #[test]
    fn size_changed_when_physical_size_differs() {
        let mut m = dp3();
        m.logical = (1920, 454, 1920, 1200);
        m.scale = 1.0;
        assert_eq!(
            area_on_dp3().staleness(&[m]),
            Some(Staleness::SizeChanged {
                from: (1680, 1050),
                to: (1920, 1200)
            })
        );
    }

    #[test]
    fn area_description() {
        assert_eq!(area_on_dp3().describe(), "1280×720 on DP-3");
    }

    #[test]
    fn disconnected_description() {
        assert_eq!(
            Staleness::Disconnected.describe("DP-3"),
            "DP-3 is not connected"
        );
    }

    #[test]
    fn size_changed_description() {
        let staleness = Staleness::SizeChanged {
            from: (1680, 1050),
            to: (1920, 1200),
        };
        assert_eq!(
            staleness.describe("DP-3"),
            "DP-3 changed from 1680×1050 to 1920×1200"
        );
    }

    #[test]
    fn scale_changed_description() {
        let staleness = Staleness::ScaleChanged {
            from: 1.05,
            to: 1.25,
        };
        assert_eq!(
            staleness.describe("DP-3"),
            "DP-3 changed scale from 1.05 to 1.25"
        );
    }
}
