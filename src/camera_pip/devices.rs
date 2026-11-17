//! Camera sources: the `Video/Source` devices of the `gst::DeviceMonitor`.

use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result};
use gst::{bus::BusWatchGuard, prelude::*};
use gtk::glib;

const VIDEO_SOURCE_CLASS: &str = "Video/Source";

/// Device properties that hold the kernel device path: `V4L2` sets the first,
/// `PipeWire` the second.
const DEVICE_PATH_PROPERTIES: [&str; 2] = ["device.path", "api.v4l2.path"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraSource {
    pub id: String,
    pub name: String,
}

/// Lists `Video/Source` devices with a monitor of its own, once. The id is
/// the device's `device.path` property when present, else its display name.
///
/// Probing the devices blocks, so the app reads a `DeviceCache` instead;
/// this is for the requirements report.
pub fn list() -> Vec<CameraSource> {
    sources(&video_devices())
}

/// The `Video/Source` devices, kept by one `gst::DeviceMonitor` that runs
/// while the cache lives, so that the menu and Preferences do not probe the
/// devices each time they open (fable code r1 #7).
#[derive(Debug)]
pub struct DeviceCache {
    monitor: gst::DeviceMonitor,
    devices: Rc<RefCell<Vec<gst::Device>>>,
    _bus_watch: Option<BusWatchGuard>,
}

impl DeviceCache {
    /// Starts the monitor, which probes the devices once, on the main
    /// context. Without a monitor the cache stays empty.
    pub fn start() -> Self {
        let monitor = gst::DeviceMonitor::new();
        monitor.add_filter(Some(VIDEO_SOURCE_CLASS), None);
        let devices = Rc::new(RefCell::new(Vec::new()));

        let bus_watch = monitor
            .bus()
            .add_watch_local({
                let devices = Rc::clone(&devices);
                move |_, message| {
                    if let Some(change) = device_change(message) {
                        apply_change(&mut devices.borrow_mut(), change);
                    }
                    glib::ControlFlow::Continue
                }
            })
            .inspect_err(|err| tracing::warn!("Failed to watch the camera devices: {:?}", err))
            .ok();

        match monitor.start() {
            Ok(()) => devices.replace(monitor.devices().into_iter().collect()),
            Err(err) => {
                tracing::warn!("Failed to start the camera device monitor: {:?}", err);
                Vec::new()
            }
        };

        Self {
            monitor,
            devices,
            _bus_watch: bus_watch,
        }
    }

    pub fn sources(&self) -> Vec<CameraSource> {
        sources(&self.devices.borrow())
    }

    /// The device for `id`, as `sources` lists it.
    pub fn device(&self, id: &str) -> Option<gst::Device> {
        self.devices
            .borrow()
            .iter()
            .find(|device| device_id(device) == id)
            .cloned()
    }
}

impl Drop for DeviceCache {
    fn drop(&mut self) {
        self.monitor.stop();
    }
}

/// A change in the devices, from a message of the monitor's bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceChange<D> {
    Added(D),
    Removed(D),
    Changed { old: D, new: D },
}

fn device_change(message: &gst::Message) -> Option<DeviceChange<gst::Device>> {
    match message.view() {
        gst::MessageView::DeviceAdded(added) => Some(DeviceChange::Added(added.device())),
        gst::MessageView::DeviceRemoved(removed) => Some(DeviceChange::Removed(removed.device())),
        gst::MessageView::DeviceChanged(changed) => {
            let (new, old) = changed.device_changed();
            Some(DeviceChange::Changed { old, new })
        }
        _ => None,
    }
}

/// Applies `change` to `devices`. The monitor may also announce the devices
/// that its first enumeration listed.
pub fn apply_change<D: PartialEq>(devices: &mut Vec<D>, change: DeviceChange<D>) {
    match change {
        DeviceChange::Added(device) => {
            if !devices.contains(&device) {
                devices.push(device);
            }
        }
        DeviceChange::Removed(device) => devices.retain(|listed| *listed != device),
        DeviceChange::Changed { old, new } => {
            if let Some(listed) = devices.iter_mut().find(|listed| **listed == old) {
                *listed = new;
            } else {
                devices.push(new);
            }
        }
    }
}

fn sources(devices: &[gst::Device]) -> Vec<CameraSource> {
    let found = devices
        .iter()
        .map(|device| {
            let source = CameraSource {
                id: device_id(device),
                name: device.display_name().to_string(),
            };
            (source, device_path(device))
        })
        .collect();

    dedupe(found)
}

/// Picks the configured id; without one, the first available. A configured
/// id that is gone gives None (codex code r1 #1).
pub fn choose<'a>(sources: &'a [CameraSource], wanted: &str) -> Option<&'a CameraSource> {
    if wanted.is_empty() {
        sources.first()
    } else {
        sources.iter().find(|source| source.id == wanted)
    }
}

/// Creates the source element of `device`.
pub fn create_element(device: &gst::Device) -> Result<gst::Element> {
    device.create_element(None).with_context(|| {
        format!(
            "Failed to create the element for camera `{}`",
            device_id(device)
        )
    })
}

/// The same webcam may be listed twice, by `PipeWire` and by `V4L2` (fable r2
/// #10): keeps the first source of each device path.
fn dedupe(found: Vec<(CameraSource, Option<String>)>) -> Vec<CameraSource> {
    let mut seen_paths = Vec::new();

    found
        .into_iter()
        .filter_map(|(source, path)| {
            if let Some(path) = path {
                if seen_paths.contains(&path) {
                    return None;
                }
                seen_paths.push(path);
            }
            Some(source)
        })
        .collect()
}

fn video_devices() -> Vec<gst::Device> {
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some(VIDEO_SOURCE_CLASS), None);

    if let Err(err) = monitor.start() {
        tracing::warn!("Failed to start the camera device monitor: {:?}", err);
        return Vec::new();
    }
    let devices = monitor.devices().into_iter().collect::<Vec<_>>();
    monitor.stop();

    devices
}

fn property(device: &gst::Device, name: &str) -> Option<String> {
    device
        .properties()?
        .get::<String>(name)
        .ok()
        .filter(|value| !value.is_empty())
}

fn device_id(device: &gst::Device) -> String {
    property(device, "device.path").unwrap_or_else(|| device.display_name().to_string())
}

fn device_path(device: &gst::Device) -> Option<String> {
    DEVICE_PATH_PROPERTIES
        .iter()
        .find_map(|name| property(device, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, name: &str) -> CameraSource {
        CameraSource {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn choose_configured_present() {
        let sources = [source("/dev/video0", "A"), source("/dev/video2", "B")];
        assert_eq!(choose(&sources, "/dev/video2"), Some(&sources[1]));
    }

    /// A saved camera that is gone is not replaced by another one, which
    /// would be recorded without the user knowing (codex code r1 #1).
    #[test]
    fn choose_configured_absent_is_none() {
        let sources = [source("/dev/video0", "A"), source("/dev/video2", "B")];
        assert_eq!(choose(&sources, "/dev/video9"), None);
    }

    #[test]
    fn choose_unconfigured_picks_first() {
        let sources = [source("/dev/video0", "A"), source("/dev/video2", "B")];
        assert_eq!(choose(&sources, ""), Some(&sources[0]));
    }

    #[test]
    fn choose_empty_is_none() {
        assert_eq!(choose(&[], "/dev/video0"), None);
        assert_eq!(choose(&[], ""), None);
    }

    #[test]
    fn device_changes_update_the_list() {
        let mut devices = vec!["webcam"];

        apply_change(&mut devices, DeviceChange::Added("capture"));
        // Already listed by the first enumeration.
        apply_change(&mut devices, DeviceChange::Added("webcam"));
        assert_eq!(devices, ["webcam", "capture"]);

        apply_change(
            &mut devices,
            DeviceChange::Changed {
                old: "capture",
                new: "capture card",
            },
        );
        assert_eq!(devices, ["webcam", "capture card"]);

        apply_change(&mut devices, DeviceChange::Removed("webcam"));
        apply_change(&mut devices, DeviceChange::Removed("gone"));
        assert_eq!(devices, ["capture card"]);
    }

    #[test]
    fn dedupe_keeps_first_per_path() {
        let found = vec![
            (source("pw-42", "Webcam"), Some("/dev/video0".to_string())),
            (
                source("/dev/video0", "Webcam"),
                Some("/dev/video0".to_string()),
            ),
            (source("OBS Virtual Camera", "OBS Virtual Camera"), None),
            (
                source("/dev/video2", "Capture"),
                Some("/dev/video2".to_string()),
            ),
            (source("Other", "Other"), None),
        ];

        assert_eq!(
            dedupe(found),
            [
                source("pw-42", "Webcam"),
                source("OBS Virtual Camera", "OBS Virtual Camera"),
                source("/dev/video2", "Capture"),
                source("Other", "Other"),
            ]
        );
    }
}
