//! A dashed border drawn on screen just outside the recorded area, as a layer
//! surface (ADR 0001).

pub mod geometry;
pub mod view;

use gtk::{gdk, prelude::*};

use self::geometry::{BorderGeometry, border_geometry};
use crate::{
    areas::{MarkedArea, MonitorInfo},
    pipeline::Crop,
    screencast_portal::{SourceType, Stream},
};

pub use self::view::AreaBorder;

/// The border of the area that `crop` cuts from `streams`, with the monitor
/// it goes on.
///
/// Only a single stream of a whole monitor has one: a window stream does not
/// say where the window is (fable r2 #6c).
pub fn for_streams(streams: &[Stream], crop: Crop) -> Option<(BorderGeometry, gdk::Monitor)> {
    let [stream] = streams else {
        tracing::debug!("No area border: {} streams", streams.len());
        return None;
    };
    if stream.source_type() != Some(SourceType::MONITOR) {
        tracing::debug!("No area border: the stream is not a monitor");
        return None;
    }

    let display = gdk::Display::default()?;
    let monitors = display
        .monitors()
        .iter::<gdk::Monitor>()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    let geometries = monitors
        .iter()
        .map(|monitor| {
            let geometry = monitor.geometry();
            (
                (
                    geometry.x(),
                    geometry.y(),
                    geometry.width(),
                    geometry.height(),
                ),
                monitor.scale(),
            )
        })
        .collect::<Vec<_>>();

    let Some(index) = stream_monitor(stream.position(), stream.size(), crop.frame, &geometries)
    else {
        tracing::warn!(
            position = ?stream.position(),
            size = ?stream.size(),
            frame = ?crop.frame,
            ?geometries,
            "No area border: cannot tell which monitor the stream captures"
        );
        return None;
    };
    let geometry = geometries[index].0;

    // The portal size and the GDK geometry are the same rectangle for a
    // whole-monitor stream; GDK is the one in logical pixels (fable r2 #6b).
    if let Some(size) = stream.size()
        && size != (geometry.2, geometry.3)
    {
        tracing::warn!(
            stream_size = ?size,
            monitor = ?geometry,
            "Stream size differs from its monitor; using the monitor's"
        );
    }

    let Some(border) = border_geometry(crop, geometry) else {
        tracing::warn!(?crop, monitor = ?geometry, "No area border: empty area");
        return None;
    };

    Some((border, monitors[index].clone()))
}

/// The border of a marked `area`, with the monitor it goes on, if that
/// monitor is among `monitors` and connected.
pub fn for_area(
    area: &MarkedArea,
    monitors: &[MonitorInfo],
) -> Option<(BorderGeometry, gdk::Monitor)> {
    let border = border_for_area(area, monitors)?;
    let display = gdk::Display::default()?;
    let monitor = crate::areas::monitors::monitor_for(&display, &area.connector)?;
    Some((border, monitor))
}

/// The border of a marked `area` on its monitor among `monitors`.
fn border_for_area(area: &MarkedArea, monitors: &[MonitorInfo]) -> Option<BorderGeometry> {
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.connector == area.connector)?;
    border_geometry(crop_for_area(area), monitor.logical)
}

/// What `videocrop` would cut to record `area` from a stream of its whole
/// monitor, at the physical size it was marked on.
fn crop_for_area(area: &MarkedArea) -> Crop {
    Crop {
        top: area.y,
        left: area.x,
        right: area.monitor_width - area.x - area.width,
        bottom: area.monitor_height - area.y - area.height,
        frame: (area.monitor_width, area.monitor_height),
    }
}

/// A monitor as its logical rectangle (x, y, w, h) and its scale factor.
type Monitor = ((i32, i32, i32, i32), f64);

/// How far, in pixels, a size may be from a monitor's to match it: fractional
/// scales round the physical size, and the frame is rounded down to even.
const SIZE_TOLERANCE: i32 = 2;

/// Index of the monitor, among `monitors`, that a whole-monitor stream
/// captures: the one at the stream `position`; without a position (KDE's
/// portal leaves it out), the only monitor, or else the only one whose size
/// matches the portal `size` (logical or physical) or the `frame` the stream
/// delivers (physical).
fn stream_monitor(
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
    frame: (i32, i32),
    monitors: &[Monitor],
) -> Option<usize> {
    if let Some(position) = position {
        return monitors
            .iter()
            .position(|&((x, y, _, _), _)| (x, y) == position);
    }
    if monitors.len() == 1 {
        return Some(0);
    }

    let near = |a: (i32, i32), b: (i32, i32)| {
        (a.0 - b.0).abs() <= SIZE_TOLERANCE && (a.1 - b.1).abs() <= SIZE_TOLERANCE
    };
    let matches = |&((_, _, width, height), scale): &Monitor| {
        let logical = (width, height);
        let physical = (
            (f64::from(width) * scale).round() as i32,
            (f64::from(height) * scale).round() as i32,
        );
        size.is_some_and(|size| size == logical || near(size, physical)) || near(frame, physical)
    };

    let mut candidates = monitors
        .iter()
        .enumerate()
        .filter(|(_, monitor)| matches(monitor))
        .map(|(index, _)| index);
    let first = candidates.next()?;
    candidates.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::areas::model::test_monitors::{dp3, edp};

    fn area_on_dp3() -> MarkedArea {
        MarkedArea {
            connector: "DP-3".into(),
            x: 105,
            y: 84,
            width: 1260,
            height: 714,
            monitor_width: 1680,
            monitor_height: 1050,
            scale: 1.05,
        }
    }

    #[test]
    fn crop_for_area_cuts_around_the_area() {
        assert_eq!(
            crop_for_area(&area_on_dp3()),
            Crop {
                top: 84,
                left: 105,
                right: 315,
                bottom: 252,
                frame: (1680, 1050),
            }
        );
    }

    #[test]
    fn border_of_an_area_is_in_logical_pixels_of_its_monitor() {
        let border = border_for_area(&area_on_dp3(), &[edp(), dp3()]).unwrap();

        // 105, 84, 1260x714 physical at 1.05 is 100, 80, 1200x680 logical.
        let (x, y, w, h) = border.area;
        for (got, want) in [(x, 100.0), (y, 80.0), (w, 1200.0), (h, 680.0)] {
            assert!((got - want).abs() < 0.01, "{:?}", border.area);
        }
        assert_eq!(border.sides, [true; 4]);
    }

    #[test]
    fn no_border_for_an_area_on_a_disconnected_monitor() {
        assert_eq!(border_for_area(&area_on_dp3(), &[edp()]), None);
    }

    const LEFT_MONITOR: Monitor = ((0, 0, 1920, 1080), 1.0);
    const RIGHT_MONITOR: Monitor = ((1920, 454, 1600, 1000), 1.05);

    // Sebastián's layout (`kscreen-doctor -o` and the Kooha log, 17/11/2026).
    const HDMI: Monitor = ((0, 0, 1920, 1080), 1.0);
    const EDP: Monitor = ((457, 1080, 1463, 914), 1.75);
    const DP: Monitor = ((1920, 454, 1600, 1000), 1.05);

    #[test]
    fn monitor_at_stream_position() {
        let monitors = [LEFT_MONITOR, RIGHT_MONITOR];

        assert_eq!(
            stream_monitor(Some((1920, 454)), None, (0, 0), &monitors),
            Some(1)
        );
        assert_eq!(
            stream_monitor(Some((0, 0)), None, (0, 0), &monitors),
            Some(0)
        );
    }

    #[test]
    fn no_monitor_at_stream_position() {
        assert_eq!(
            stream_monitor(Some((10, 10)), None, (0, 0), &[LEFT_MONITOR]),
            None
        );
    }

    #[test]
    fn unknown_position_with_one_monitor() {
        assert_eq!(
            stream_monitor(None, None, (0, 0), &[RIGHT_MONITOR]),
            Some(0)
        );
    }

    #[test]
    fn unknown_position_and_size_with_two_monitors() {
        assert_eq!(
            stream_monitor(None, None, (0, 0), &[LEFT_MONITOR, RIGHT_MONITOR]),
            None
        );
    }

    /// KDE's portal leaves out the position: the frame size, in physical
    /// pixels, tells the three monitors apart.
    #[test]
    fn unknown_position_matched_by_frame_size() {
        let monitors = [HDMI, EDP, DP];

        assert_eq!(stream_monitor(None, None, (2560, 1600), &monitors), Some(1));
        assert_eq!(stream_monitor(None, None, (1680, 1050), &monitors), Some(2));
        assert_eq!(stream_monitor(None, None, (1920, 1080), &monitors), Some(0));
    }

    #[test]
    fn unknown_position_matched_by_portal_size() {
        let monitors = [HDMI, EDP, DP];

        // Logical, as the portal spec says, or physical, as some portals do.
        assert_eq!(
            stream_monitor(None, Some((1600, 1000)), (0, 0), &monitors),
            Some(2)
        );
        assert_eq!(
            stream_monitor(None, Some((2560, 1600)), (0, 0), &monitors),
            Some(1)
        );
    }

    #[test]
    fn same_size_monitors_stay_ambiguous() {
        let twin = ((1920, 0, 1920, 1080), 1.0);

        assert_eq!(
            stream_monitor(None, None, (1920, 1080), &[HDMI, twin]),
            None
        );
    }
}
