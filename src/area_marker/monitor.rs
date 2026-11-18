//! The monitors the marker draws on, and their logical↔physical conversions.
//!
//! Private stand-ins for `MonitorInfo::physical_size`, `to_physical` and
//! `to_logical` and for `areas::monitors::current`, with the same formula
//! (`round(logical × scale)`); they go away when the two are unified.

use gtk::{gdk, prelude::*};

use crate::areas::MonitorInfo;

/// The monitor size in physical (video) pixels.
pub fn physical_size(monitor: &MonitorInfo) -> (i32, i32) {
    let (_, _, width, height) = monitor.logical;
    to_physical(monitor, f64::from(width), f64::from(height))
}

/// A point local to the monitor, from logical to physical pixels.
pub fn to_physical(monitor: &MonitorInfo, x: f64, y: f64) -> (i32, i32) {
    (
        (x * monitor.scale).round() as i32,
        (y * monitor.scale).round() as i32,
    )
}

/// A point local to the monitor, from physical to logical pixels.
pub fn to_logical(monitor: &MonitorInfo, x: i32, y: i32) -> (f64, f64) {
    (f64::from(x) / monitor.scale, f64::from(y) / monitor.scale)
}

/// The connected monitors, with what the marker needs of each.
pub fn current(display: &gdk::Display) -> Vec<(gdk::Monitor, MonitorInfo)> {
    display
        .monitors()
        .iter::<gdk::Monitor>()
        .filter_map(Result::ok)
        .filter(|monitor| monitor.is_valid())
        .enumerate()
        .map(|(index, monitor)| {
            let geometry = monitor.geometry();
            let info = MonitorInfo {
                connector: monitor
                    .connector()
                    .map_or_else(|| format!("unknown-{index}"), String::from),
                logical: (
                    geometry.x(),
                    geometry.y(),
                    geometry.width(),
                    geometry.height(),
                ),
                scale: monitor.scale(),
            };
            (monitor, info)
        })
        .collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Sebastián's laptop panel, as GDK reports it.
    pub fn edp() -> MonitorInfo {
        MonitorInfo {
            connector: "eDP-1".into(),
            logical: (457, 1080, 1463, 914),
            scale: 1.75,
        }
    }

    pub fn dp3() -> MonitorInfo {
        MonitorInfo {
            connector: "DP-3".into(),
            logical: (1920, 454, 1600, 1000),
            scale: 1.05,
        }
    }

    #[test]
    fn physical_size_rounds_logical_times_scale() {
        // 914 × 1.75 = 1599.5
        assert_eq!(physical_size(&edp()), (2560, 1600));
        assert_eq!(physical_size(&dp3()), (1680, 1050));
    }

    #[test]
    fn conversions_are_local_to_the_monitor() {
        let monitor = dp3();
        assert_eq!(to_physical(&monitor, 100.0, 80.0), (105, 84));
        let (x, y) = to_logical(&monitor, 105, 84);
        assert!((x - 100.0).abs() < 0.01 && (y - 80.0).abs() < 0.01);
    }
}
