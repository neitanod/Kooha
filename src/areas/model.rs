//! Marked areas: a rectangle on one monitor, in physical (video) pixels.

use gettextrs::gettext;
use serde::{Deserialize, Serialize};

use crate::{
    area_marker::geometry::{MIN_HEIGHT, MIN_WIDTH},
    i18n::gettext_f,
};

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
    /// `None` if it can still be recorded. An invalid area, as one edited
    /// by hand in the file, is never recorded, whatever the monitors.
    pub fn staleness(&self, monitors: &[MonitorInfo]) -> Option<Staleness> {
        if let Err(reason) = self.validate() {
            return Some(Staleness::Invalid(reason));
        }

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

    /// Whether the geometry can be recorded on the monitor it was marked
    /// on: inside it, even-sized and at least the minimum, with a usable
    /// scale. The marker never gives anything else; the file may (codex
    /// code r1 #6, fable code r1 #9).
    pub fn validate(&self) -> Result<(), InvalidArea> {
        if self.x < 0 || self.y < 0 {
            return Err(InvalidArea::NegativePosition);
        }
        if self.width < MIN_WIDTH || self.height < MIN_HEIGHT {
            return Err(InvalidArea::TooSmall);
        }
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(InvalidArea::OddSize);
        }
        if !ends_within(self.x, self.width, self.monitor_width)
            || !ends_within(self.y, self.height, self.monitor_height)
        {
            return Err(InvalidArea::OutsideMonitor);
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return Err(InvalidArea::BadScale);
        }
        Ok(())
    }

    /// Whether the rectangle, non-empty, lies inside the current physical
    /// size of `monitor`.
    pub fn fits(&self, monitor: &MonitorInfo) -> bool {
        let (monitor_width, monitor_height) = monitor.physical_size();
        self.x >= 0
            && self.y >= 0
            && self.width > 0
            && self.height > 0
            && ends_within(self.x, self.width, monitor_width)
            && ends_within(self.y, self.height, monitor_height)
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

/// Whether `start + length` is at most `limit`, without overflowing: the
/// numbers may come from a file edited by hand.
pub fn ends_within(start: i32, length: i32, limit: i32) -> bool {
    start.checked_add(length).is_some_and(|end| end <= limit)
}

/// A marked area with a name, in the saved list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedArea {
    pub name: String,
    #[serde(flatten)]
    pub area: MarkedArea,
}

/// The area chosen to record: a saved one, with its name, or the last
/// marked one, with no name.
#[derive(Debug, Clone, PartialEq)]
pub struct ChosenArea {
    pub name: Option<String>,
    pub area: MarkedArea,
}

/// What is wrong with the geometry of an area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidArea {
    /// `x` or `y` is negative.
    NegativePosition,
    /// The width or the height is odd.
    OddSize,
    /// Under the minimum size of the marker.
    TooSmall,
    /// Past the right or the bottom edge of its monitor.
    OutsideMonitor,
    /// The scale is not a positive number.
    BadScale,
}

/// Why an area can't be recorded any more.
#[derive(Debug, Clone, PartialEq)]
pub enum Staleness {
    /// The geometry itself can't be recorded.
    Invalid(InvalidArea),
    Disconnected,
    SizeChanged {
        from: (i32, i32),
        to: (i32, i32),
    },
    ScaleChanged {
        from: f64,
        to: f64,
    },
}

impl Staleness {
    /// Why an area on `connector` is stale, like `DP-3 is not connected`.
    pub fn describe(&self, connector: &str) -> String {
        match self {
            Self::Invalid(reason) => reason.describe(connector),
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

impl InvalidArea {
    /// What is wrong, like `the area goes past the edge of DP-3`.
    pub fn describe(self, connector: &str) -> String {
        match self {
            Self::NegativePosition => gettext_f(
                "the area starts outside {connector}",
                &[("connector", connector)],
            ),
            Self::OddSize => gettext("the area has an odd width or height"),
            Self::TooSmall => gettext_f(
                "the area is smaller than {size}",
                &[("size", &format!("{MIN_WIDTH}×{MIN_HEIGHT}"))],
            ),
            Self::OutsideMonitor => gettext_f(
                "the area goes past the edge of {connector}",
                &[("connector", connector)],
            ),
            Self::BadScale => gettext_f(
                "the scale of {connector} in the list is not valid",
                &[("connector", connector)],
            ),
        }
    }
}

/// Sebastián's monitors, as GDK reports them (`kscreen-doctor -o`,
/// 17/11/2026), for the tests of every module.
#[cfg(test)]
pub mod test_monitors {
    use super::MonitorInfo;

    /// The notebook panel: 2560x1600 at 1.75.
    pub fn edp() -> MonitorInfo {
        MonitorInfo {
            connector: "eDP-1".into(),
            logical: (457, 1080, 1463, 914),
            scale: 1.75,
        }
    }

    /// 1680x1050 at 1.05.
    pub fn dp3() -> MonitorInfo {
        MonitorInfo {
            connector: "DP-3".into(),
            logical: (1920, 454, 1600, 1000),
            scale: 1.05,
        }
    }

    /// 1920x1080 at 1.
    pub fn hdmi() -> MonitorInfo {
        MonitorInfo {
            connector: "HDMI-A-1".into(),
            logical: (0, 0, 1920, 1080),
            scale: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        test_monitors::{dp3, edp},
        *,
    };

    #[test]
    fn physical_size_rounds_logical_times_scale() {
        // 1463 * 1.75 = 2560.25, 914 * 1.75 = 1599.5
        assert_eq!(edp().physical_size(), (2560, 1600));
        assert_eq!(dp3().physical_size(), (1680, 1050));
    }

    #[test]
    fn physical_size_rounds_to_nearest() {
        // 915 * 1.75 = 1601.25: one logical pixel more than GDK reports for
        // the panel gives 1601, within the size tolerance of 1600.
        let mut m = edp();
        m.logical.3 = 915;
        assert_eq!(m.physical_size(), (2560, 1601));
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
    fn invalid_comes_before_disconnected() {
        let area = MarkedArea {
            x: -2,
            ..area_on_dp3()
        };
        assert_eq!(
            area.staleness(&[]),
            Some(Staleness::Invalid(InvalidArea::NegativePosition))
        );
    }

    #[test]
    fn a_negative_position_is_invalid() {
        for area in [
            MarkedArea {
                x: -2,
                ..area_on_dp3()
            },
            MarkedArea {
                y: -1,
                ..area_on_dp3()
            },
        ] {
            assert_eq!(
                area.staleness(&[dp3()]),
                Some(Staleness::Invalid(InvalidArea::NegativePosition)),
                "{area:?}"
            );
        }
    }

    #[test]
    fn an_odd_size_is_invalid() {
        for area in [
            MarkedArea {
                width: 1281,
                ..area_on_dp3()
            },
            MarkedArea {
                height: 721,
                ..area_on_dp3()
            },
        ] {
            assert_eq!(
                area.staleness(&[dp3()]),
                Some(Staleness::Invalid(InvalidArea::OddSize)),
                "{area:?}"
            );
        }
    }

    #[test]
    fn a_size_under_the_minimum_is_invalid() {
        for area in [
            MarkedArea {
                width: 158,
                ..area_on_dp3()
            },
            MarkedArea {
                height: 118,
                ..area_on_dp3()
            },
            MarkedArea {
                width: 0,
                height: 0,
                ..area_on_dp3()
            },
            MarkedArea {
                width: -200,
                ..area_on_dp3()
            },
        ] {
            assert_eq!(
                area.staleness(&[dp3()]),
                Some(Staleness::Invalid(InvalidArea::TooSmall)),
                "{area:?}"
            );
        }

        let smallest = MarkedArea {
            width: 160,
            height: 120,
            ..area_on_dp3()
        };
        assert_eq!(smallest.staleness(&[dp3()]), None);
    }

    #[test]
    fn an_area_past_its_monitor_is_invalid() {
        for area in [
            MarkedArea {
                x: 402,
                ..area_on_dp3()
            },
            MarkedArea {
                y: 332,
                ..area_on_dp3()
            },
            // Overflows i32: checked, not wrapped around.
            MarkedArea {
                x: i32::MAX - 100,
                ..area_on_dp3()
            },
            MarkedArea {
                y: i32::MAX - 100,
                ..area_on_dp3()
            },
        ] {
            assert_eq!(
                area.staleness(&[dp3()]),
                Some(Staleness::Invalid(InvalidArea::OutsideMonitor)),
                "{area:?}"
            );
        }

        // Ends exactly at 1680×1050.
        let edge = MarkedArea {
            x: 400,
            y: 330,
            ..area_on_dp3()
        };
        assert_eq!(edge.staleness(&[dp3()]), None);
    }

    #[test]
    fn a_scale_not_positive_and_finite_is_invalid() {
        for scale in [0.0, -1.05, f64::NAN, f64::INFINITY] {
            let area = MarkedArea {
                scale,
                ..area_on_dp3()
            };
            assert_eq!(area.validate(), Err(InvalidArea::BadScale), "scale {scale}");
            assert_eq!(
                area.staleness(&[dp3()]),
                Some(Staleness::Invalid(InvalidArea::BadScale)),
                "scale {scale}"
            );
        }
        assert_eq!(area_on_dp3().validate(), Ok(()));
    }

    #[test]
    fn invalid_descriptions() {
        let describe = |reason| Staleness::Invalid(reason).describe("DP-3");
        assert_eq!(
            describe(InvalidArea::NegativePosition),
            "the area starts outside DP-3"
        );
        assert_eq!(
            describe(InvalidArea::OddSize),
            "the area has an odd width or height"
        );
        assert_eq!(
            describe(InvalidArea::TooSmall),
            "the area is smaller than 160×120"
        );
        assert_eq!(
            describe(InvalidArea::OutsideMonitor),
            "the area goes past the edge of DP-3"
        );
        assert_eq!(
            describe(InvalidArea::BadScale),
            "the scale of DP-3 in the list is not valid"
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

    #[test]
    fn fits_inside_the_physical_monitor() {
        let area = area_on_dp3(); // 100,80 1280x720 on 1680x1050
        assert!(area.fits(&dp3()));

        let edge = MarkedArea {
            x: 400,
            y: 330,
            ..area_on_dp3()
        }; // ends exactly at 1680x1050
        assert!(edge.fits(&dp3()));
    }

    #[test]
    fn does_not_fit_past_the_edges() {
        let right = MarkedArea {
            x: 401,
            ..area_on_dp3()
        };
        assert!(!right.fits(&dp3()));

        let bottom = MarkedArea {
            y: 331,
            ..area_on_dp3()
        };
        assert!(!bottom.fits(&dp3()));

        let negative = MarkedArea {
            x: -1,
            ..area_on_dp3()
        };
        assert!(!negative.fits(&dp3()));

        let empty = MarkedArea {
            width: 0,
            ..area_on_dp3()
        };
        assert!(!empty.fits(&dp3()));
    }

    #[test]
    fn does_not_fit_when_the_end_overflows() {
        let area = MarkedArea {
            x: i32::MAX - 100,
            ..area_on_dp3()
        };
        assert!(!area.fits(&dp3()));
    }

    #[test]
    fn does_not_fit_a_smaller_monitor() {
        let mut m = dp3();
        m.logical = (1920, 454, 1280, 800);
        m.scale = 1.0;
        assert!(!area_on_dp3().fits(&m));
    }
}
