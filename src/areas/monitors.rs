//! The connected monitors, as GDK reports them.
//!
//! No unit tests: everything here needs a display. The manual test of the
//! integration covers it.

use std::rc::Rc;

use gtk::{gdk, prelude::*};

use super::model::MonitorInfo;

/// The valid monitors of `display`, in GDK's order.
pub fn current(display: &gdk::Display) -> Vec<MonitorInfo> {
    current_with_gdk(display)
        .into_iter()
        .map(|(_, info)| info)
        .collect()
}

/// The valid monitors of `display`, in GDK's order, each with its
/// `gdk::Monitor`.
pub fn current_with_gdk(display: &gdk::Display) -> Vec<(gdk::Monitor, MonitorInfo)> {
    valid(display)
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            let geometry = monitor.geometry();
            let info = MonitorInfo {
                connector: connector(&monitor, index),
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

/// The monitor of `display` named `connector`, as `current` names them.
pub fn monitor_for(display: &gdk::Display, connector: &str) -> Option<gdk::Monitor> {
    valid(display)
        .into_iter()
        .enumerate()
        .find(|(index, monitor)| self::connector(monitor, *index) == connector)
        .map(|(_, monitor)| monitor)
}

/// Calls `callback` whenever a monitor is connected or disconnected, or
/// changes geometry or scale, so that stale areas are recomputed. A scale
/// change on a monitor that stays connected only shows up as a property
/// notification, never in the list.
///
/// The handlers live as long as `display`.
pub fn watch(display: &gdk::Display, callback: impl Fn() + 'static) {
    let callback: Rc<dyn Fn()> = Rc::new(callback);

    let list = display.monitors();
    for monitor in list.iter::<gdk::Monitor>().filter_map(Result::ok) {
        watch_monitor(&monitor, &callback);
    }

    list.connect_items_changed(move |list, position, _removed, added| {
        for index in position..position + added {
            if let Some(monitor) = list.item(index).and_downcast::<gdk::Monitor>() {
                watch_monitor(&monitor, &callback);
            }
        }
        callback();
    });
}

fn watch_monitor(monitor: &gdk::Monitor, callback: &Rc<dyn Fn()>) {
    let on_geometry = callback.clone();
    monitor.connect_geometry_notify(move |_| on_geometry());
    let on_scale = callback.clone();
    monitor.connect_scale_notify(move |_| on_scale());
}

fn valid(display: &gdk::Display) -> Vec<gdk::Monitor> {
    display
        .monitors()
        .iter::<gdk::Monitor>()
        .filter_map(Result::ok)
        .filter(|monitor| monitor.is_valid())
        .collect()
}

/// The connector name, or `unknown-<index>` when GDK doesn't know it.
fn connector(monitor: &gdk::Monitor, index: usize) -> String {
    monitor.connector().map_or_else(
        || format!("unknown-{index}"),
        |connector| connector.to_string(),
    )
}
