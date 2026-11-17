//! The camera mode the `PiP` asks for, and why a camera failed.

use gettextrs::gettext;
use gtk::glib::{self, translate::IntoGlib};

/// The `PiP` is at most a few hundred pixels tall, so 720p is plenty.
const TARGET_HEIGHT: i32 = 720;
const TARGET_FRAMERATE: gst::Fraction = gst::Fraction::from_integer(30);
/// Below this the `PiP` moves by jumps.
const MIN_FRAMERATE: gst::Fraction = gst::Fraction::from_integer(24);

/// The mode to ask `device_caps` for, as caps for a `capsfilter` before a
/// `decodebin` (fable code r1 #2, codex code r1 #3): raw or JPEG in system
/// memory, at least 24 fps, the height closest to 720, raw on a tie. Without
/// such a mode, any mode the device has.
pub fn preferred_caps(device_caps: &gst::Caps) -> gst::Caps {
    device_caps
        .iter_with_features()
        .filter(|(structure, features)| is_usable(structure, features))
        .filter_map(|(structure, _)| Candidate::fixed_from(structure))
        .filter(|candidate| candidate.framerate >= MIN_FRAMERATE)
        .max_by_key(Candidate::rank)
        .map_or_else(gst::Caps::new_any, |candidate| {
            gst::Caps::builder_full()
                .structure(candidate.structure)
                .build()
        })
}

/// Raw frames in system memory, which `videoconvert` takes, or JPEG.
fn is_usable(structure: &gst::StructureRef, features: &gst::CapsFeaturesRef) -> bool {
    let in_system_memory = !features.is_any()
        && features
            .iter()
            .all(|feature| *feature == *gst::CAPS_FEATURE_MEMORY_SYSTEM_MEMORY);

    match structure.name().as_str() {
        "video/x-raw" => {
            in_system_memory && structure.get::<&str>("format").ok() != Some("DMA_DRM")
        }
        "image/jpeg" => in_system_memory,
        _ => false,
    }
}

struct Candidate {
    structure: gst::Structure,
    height: i32,
    framerate: gst::Fraction,
}

impl Candidate {
    /// `structure` with its size and framerate fixed to the ones closest to
    /// 720p at 30 fps, if it has them.
    fn fixed_from(structure: &gst::StructureRef) -> Option<Self> {
        let mut structure = structure.to_owned();
        structure.fixate_field_nearest_int("height", TARGET_HEIGHT);
        let height = structure.get::<i32>("height").ok()?;
        structure.fixate_field_nearest_int("width", height * 16 / 9);
        structure.fixate_field_nearest_fraction("framerate", TARGET_FRAMERATE);
        let framerate = structure.get::<gst::Fraction>("framerate").ok()?;

        Some(Self {
            structure,
            height,
            framerate,
        })
    }

    /// Higher is better.
    fn rank(&self) -> (i32, bool, gst::Fraction) {
        (
            -(self.height - TARGET_HEIGHT).abs(),
            self.structure.name() == "video/x-raw",
            self.framerate,
        )
    }
}

/// Why the camera was turned off, as the notification says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraFailure {
    /// There is no camera at all.
    NotFound,
    /// The chosen camera is gone.
    Disconnected,
    /// The camera is there but could not be started.
    OpenFailed,
    /// No mode of the camera could be shown.
    UnsupportedFormat,
}

impl CameraFailure {
    pub fn message(self) -> String {
        match self {
            Self::NotFound => gettext("No camera found"),
            Self::Disconnected => gettext("Camera disconnected"),
            Self::OpenFailed => gettext("Could not open the camera"),
            Self::UnsupportedFormat => gettext("Camera format not supported"),
        }
    }
}

/// The failure behind an error of the camera pipeline. `flow_return` is the
/// `flow-return` detail of the message, which says `not-negotiated` for a
/// format no element took; `was_playing`, whether the camera had started.
pub fn camera_failure(
    error: &glib::Error,
    flow_return: Option<i32>,
    was_playing: bool,
) -> CameraFailure {
    let is_format_error = flow_return == Some(gst::FlowReturn::NotNegotiated.into_glib())
        || [
            gst::StreamError::Format,
            gst::StreamError::WrongType,
            gst::StreamError::CodecNotFound,
            gst::StreamError::Decode,
            gst::StreamError::TypeNotFound,
            gst::StreamError::NotImplemented,
        ]
        .into_iter()
        .any(|code| error.matches(code))
        || error.matches(gst::CoreError::Negotiation)
        || error.matches(gst::CoreError::MissingPlugin);

    if is_format_error {
        CameraFailure::UnsupportedFormat
    } else if was_playing {
        CameraFailure::Disconnected
    } else {
        CameraFailure::OpenFailed
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    /// What the `GStreamer` device monitor lists for the built-in camera of
    /// Sebastián's notebook through `PipeWire` (fable code r1 #2): JPEG at
    /// 30 fps in every size, raw YUY2 at 5 fps in 1080p. The listing was cut
    /// after twelve of its 27 modes.
    const PIPEWIRE_CAMERA: &str = "\
        image/jpeg, width=(int)1920, height=(int)1080, framerate=(fraction)30/1; \
        image/jpeg, width=(int)1280, height=(int)720, framerate=(fraction)30/1; \
        image/jpeg, width=(int)960, height=(int)540, framerate=(fraction)30/1; \
        image/jpeg, width=(int)848, height=(int)480, framerate=(fraction)30/1; \
        image/jpeg, width=(int)640, height=(int)480, framerate=(fraction)30/1; \
        image/jpeg, width=(int)640, height=(int)360, framerate=(fraction)30/1; \
        image/jpeg, width=(int)352, height=(int)288, framerate=(fraction)30/1; \
        image/jpeg, width=(int)320, height=(int)240, framerate=(fraction)30/1; \
        image/jpeg, width=(int)320, height=(int)180, framerate=(fraction)30/1; \
        video/x-raw, drm-format=(string)YUYV, format=(string)DMA_DRM, width=(int)1920, height=(int)1080, framerate=(fraction)5/1; \
        video/x-raw, format=(string)YUY2, width=(int)1920, height=(int)1080, framerate=(fraction)5/1; \
        video/x-raw, drm-format=(string)YUYV, format=(string)DMA_DRM, width=(int)1280, height=(int)720, framerate=(fraction)10/1";

    /// The same camera through `V4L2`: raw first, slow in the big sizes.
    /// The listing was cut after twelve of its 21 modes.
    const V4L2_CAMERA: &str = "\
        video/x-raw, format=(string)DMA_DRM, drm-format=(string)YUYV, width=(int)1920, height=(int)1080, framerate=(fraction)5/1; \
        video/x-raw, format=(string)DMA_DRM, drm-format=(string)YUYV, width=(int)1280, height=(int)720, framerate=(fraction)10/1; \
        video/x-raw, format=(string)DMA_DRM, drm-format=(string)YUYV, width=(int)960, height=(int)540, framerate=(fraction)15/1; \
        video/x-raw, format=(string)DMA_DRM, drm-format=(string)YUYV, width=(int)848, height=(int)480, framerate=(fraction)20/1; \
        video/x-raw, format=(string)DMA_DRM, drm-format=(string)YUYV, width=(int)640, height=(int){ 480, 360 }, framerate=(fraction)30/1; \
        video/x-raw, format=(string)DMA_DRM, drm-format=(string)YUYV, width=(int)352, height=(int)288, framerate=(fraction)30/1; \
        video/x-raw, format=(string)DMA_DRM, drm-format=(string)YUYV, width=(int)320, height=(int){ 240, 180 }, framerate=(fraction)30/1; \
        image/jpeg, parsed=(boolean)true, width=(int)1920, height=(int)1080, framerate=(fraction)30/1; \
        image/jpeg, parsed=(boolean)true, width=(int)1280, height=(int)720, framerate=(fraction)30/1; \
        image/jpeg, parsed=(boolean)true, width=(int)960, height=(int)540, framerate=(fraction)30/1; \
        image/jpeg, parsed=(boolean)true, width=(int)848, height=(int)480, framerate=(fraction)30/1; \
        image/jpeg, parsed=(boolean)true, width=(int)640, height=(int){ 480, 360 }, framerate=(fraction)30/1";

    fn caps(description: &str) -> gst::Caps {
        gst::init().unwrap();
        gst::Caps::from_str(description).unwrap()
    }

    #[test]
    fn pipewire_camera_gets_jpeg_720p_at_30() {
        assert_eq!(
            preferred_caps(&caps(PIPEWIRE_CAMERA)),
            caps("image/jpeg, width=(int)1280, height=(int)720, framerate=(fraction)30/1")
        );
    }

    #[test]
    fn v4l2_camera_gets_jpeg_720p_at_30() {
        assert_eq!(
            preferred_caps(&caps(V4L2_CAMERA)),
            caps(
                "image/jpeg, parsed=(boolean)true, width=(int)1280, height=(int)720, \
                 framerate=(fraction)30/1"
            )
        );
    }

    #[test]
    fn fluid_mode_wins_over_closer_size() {
        let device = caps(
            "video/x-raw, format=(string)YUY2, width=(int)1280, height=(int)720, framerate=(fraction)10/1; \
             video/x-raw, format=(string)YUY2, width=(int)640, height=(int){ 480, 360 }, framerate=(fraction){ 30/1, 15/1 }",
        );

        assert_eq!(
            preferred_caps(&device),
            caps(
                "video/x-raw, format=(string)YUY2, width=(int)640, height=(int)480, \
                 framerate=(fraction)30/1"
            )
        );
    }

    #[test]
    fn raw_wins_a_tie_with_jpeg() {
        let device = caps(
            "image/jpeg, width=(int)1280, height=(int)720, framerate=(fraction)30/1; \
             video/x-raw, format=(string)NV12, width=(int)1280, height=(int)720, framerate=(fraction)30/1",
        );

        assert_eq!(
            preferred_caps(&device),
            caps(
                "video/x-raw, format=(string)NV12, width=(int)1280, height=(int)720, \
                 framerate=(fraction)30/1"
            )
        );
    }

    #[test]
    fn ranges_are_fixed_near_720p_at_30() {
        let device = caps(
            "video/x-raw, format=(string)I420, width=(int)[ 16, 4096 ], height=(int)[ 16, 4096 ], \
             framerate=(fraction)[ 1/1, 60/1 ]",
        );

        assert_eq!(
            preferred_caps(&device),
            caps(
                "video/x-raw, format=(string)I420, width=(int)1280, height=(int)720, \
                 framerate=(fraction)30/1"
            )
        );
    }

    #[test]
    fn without_a_fluid_mode_any_mode_goes() {
        let device = caps(
            "video/x-raw, format=(string)YUY2, width=(int)1920, height=(int)1080, framerate=(fraction)5/1; \
             video/x-h264, width=(int)1280, height=(int)720, framerate=(fraction)30/1",
        );
        assert!(preferred_caps(&device).is_any());

        let dmabuf_only = caps(
            "video/x-raw(memory:DMABuf), format=(string)DMA_DRM, drm-format=(string)YUYV, \
             width=(int)1280, height=(int)720, framerate=(fraction)30/1",
        );
        assert!(preferred_caps(&dmabuf_only).is_any());
    }

    #[test]
    fn camera_failures() {
        let not_negotiated = gst::FlowReturn::NotNegotiated.into_glib();
        let failed = glib::Error::new(gst::StreamError::Failed, "Internal data stream error.");

        assert_eq!(
            camera_failure(&failed, Some(not_negotiated), false),
            CameraFailure::UnsupportedFormat
        );
        assert_eq!(
            camera_failure(
                &glib::Error::new(gst::StreamError::CodecNotFound, "No decoder"),
                None,
                false
            ),
            CameraFailure::UnsupportedFormat
        );
        assert_eq!(
            camera_failure(
                &glib::Error::new(gst::CoreError::Negotiation, "No format"),
                None,
                true
            ),
            CameraFailure::UnsupportedFormat
        );
        assert_eq!(
            camera_failure(
                &glib::Error::new(gst::ResourceError::Busy, "Device busy"),
                None,
                false
            ),
            CameraFailure::OpenFailed
        );
        assert_eq!(
            camera_failure(&failed, None, false),
            CameraFailure::OpenFailed
        );
        assert_eq!(
            camera_failure(
                &glib::Error::new(gst::ResourceError::Read, "Could not read"),
                None,
                true
            ),
            CameraFailure::Disconnected
        );
    }

    #[test]
    fn failure_messages() {
        assert_eq!(CameraFailure::NotFound.message(), "No camera found");
        assert_eq!(CameraFailure::Disconnected.message(), "Camera disconnected");
        assert_eq!(
            CameraFailure::OpenFailed.message(),
            "Could not open the camera"
        );
        assert_eq!(
            CameraFailure::UnsupportedFormat.message(),
            "Camera format not supported"
        );
    }
}
