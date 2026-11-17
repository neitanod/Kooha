//! A dashed border drawn on screen just outside the recorded area, as a layer
//! surface (ADR 0001).

pub mod geometry;
pub mod view;

use gtk::{gdk, prelude::*};

use self::geometry::{BorderGeometry, border_geometry};
use crate::{
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
                geometry.x(),
                geometry.y(),
                geometry.width(),
                geometry.height(),
            )
        })
        .collect::<Vec<_>>();

    let Some(index) = stream_monitor(stream.position(), &geometries) else {
        tracing::warn!(
            position = ?stream.position(),
            ?geometries,
            "No area border: cannot tell which monitor the stream captures"
        );
        return None;
    };
    let geometry = geometries[index];

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

/// Index of the monitor, among `monitors` (x, y, w, h), that a whole-monitor
/// stream at `position` captures.
fn stream_monitor(
    position: Option<(i32, i32)>,
    monitors: &[(i32, i32, i32, i32)],
) -> Option<usize> {
    match position {
        Some(position) => monitors.iter().position(|&(x, y, _, _)| (x, y) == position),
        None if monitors.len() == 1 => Some(0),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT_MONITOR: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
    const RIGHT_MONITOR: (i32, i32, i32, i32) = (1920, 454, 1600, 1000);

    #[test]
    fn monitor_at_stream_position() {
        let monitors = [LEFT_MONITOR, RIGHT_MONITOR];

        assert_eq!(stream_monitor(Some((1920, 454)), &monitors), Some(1));
        assert_eq!(stream_monitor(Some((0, 0)), &monitors), Some(0));
    }

    #[test]
    fn no_monitor_at_stream_position() {
        assert_eq!(stream_monitor(Some((10, 10)), &[LEFT_MONITOR]), None);
    }

    #[test]
    fn unknown_position_with_one_monitor() {
        assert_eq!(stream_monitor(None, &[RIGHT_MONITOR]), Some(0));
    }

    #[test]
    fn unknown_position_with_two_monitors() {
        assert_eq!(stream_monitor(None, &[LEFT_MONITOR, RIGHT_MONITOR]), None);
    }
}
