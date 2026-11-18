use anyhow::{Context, Ok, Result, bail, ensure};
use gst::prelude::*;
use gtk::graphene::Rect;

use std::{
    os::unix::io::RawFd,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    area_selector::SelectAreaData,
    device::{self, DeviceClass},
    encoder::H264Encoder,
    pause_gate::{FlashColor, PauseGate},
    profile::Profile,
    screencast_portal::Stream,
};

const AUDIO_SAMPLE_RATE: i32 = 48_000;

/// The most raw video the queue before the encoder holds.
const VIDEOENC_QUEUE_MAX_BYTES: u32 = 256 * 1024 * 1024;

#[derive(Debug)]
#[must_use]
pub struct PipelineBuilder {
    file_path: PathBuf,
    framerate: gst::Fraction,
    profile: Profile,
    fd: RawFd,
    streams: Vec<Stream>,
    record_desktop_audio: bool,
    record_microphone: bool,
    select_area_data: Option<SelectAreaData>,
    area_crop: Option<AreaCrop>,
    pause_gate: PauseGate,
    pause_flash: Option<FlashColor>,
    encoder: H264Encoder,
}

impl PipelineBuilder {
    pub fn new(
        file_path: PathBuf,
        framerate: gst::Fraction,
        profile: Profile,
        fd: RawFd,
        streams: Vec<Stream>,
    ) -> Self {
        Self {
            file_path,
            framerate,
            profile,
            fd,
            streams,
            record_desktop_audio: false,
            record_microphone: false,
            select_area_data: None,
            area_crop: None,
            pause_gate: PauseGate::default(),
            pause_flash: None,
            encoder: H264Encoder::Cpu,
        }
    }

    pub fn record_desktop_audio(&mut self, record_desktop_audio: bool) -> &mut Self {
        self.record_desktop_audio = record_desktop_audio;
        self
    }

    pub fn record_microphone(&mut self, record_microphone: bool) -> &mut Self {
        self.record_microphone = record_microphone;
        self
    }

    pub fn select_area_data(&mut self, data: SelectAreaData) -> &mut Self {
        self.select_area_data = Some(data);
        self
    }

    /// Records only `area` of the stream. It takes precedence over
    /// `select_area_data`.
    // The recording starts calling it once marked areas are wired in.
    #[allow(dead_code)]
    pub fn area_crop(&mut self, area: AreaCrop) -> &mut Self {
        self.area_crop = Some(area);
        self
    }

    pub fn pause_gate(&mut self, gate: PauseGate) -> &mut Self {
        self.pause_gate = gate;
        self
    }

    /// The color of the resume flash, or `None` for no flash.
    pub fn pause_flash(&mut self, pause_flash: Option<FlashColor>) -> &mut Self {
        self.pause_flash = pause_flash;
        self
    }

    /// The H.264 encoder, for the profiles that use one. The CPU by default.
    pub fn encoder(&mut self, encoder: H264Encoder) -> &mut Self {
        self.encoder = encoder;
        self
    }

    /// The size of the frames the encoder gets, or `None` when a stream does
    /// not tell its size.
    pub fn output_size(&self) -> Option<(i32, i32)> {
        if let Some(area) = self.area_crop {
            return Some((area.width, area.height));
        }

        if let Some(ref data) = self.select_area_data {
            return Some(scaled_caps_size(cropped_size(&compute_crop(data))));
        }

        // The compositor puts the streams side by side.
        self.streams
            .iter()
            .try_fold((0, 0), |(width, height), stream| {
                let (stream_width, stream_height) = stream.size()?;
                Some((width + stream_width, height.max(stream_height)))
            })
    }

    /// Builds the pipeline.
    ///
    /// With an area crop, `videocrop` cuts the area in stream pixels and
    /// nothing is scaled. With `select_area_data` (the upstream selector),
    /// `videocrop` cuts first and `videoscale` makes the result even.
    ///
    ///                  (If has area_crop or select_area_data)  (If pause_flash)
    ///                        |             |                           |
    ///                        v             v                           v
    /// pipewiresrc-bin -> videocrop -> [videoscale -> caps] -> gate -> videobalance -> queue -> |
    ///                                                                                         | -> profile.attach -> filesink
    ///                                                         pulsesrc-bin -> gate -> queue -> |
    pub fn build(&self) -> Result<gst::Pipeline> {
        tracing::debug!(
            file_path = %self.file_path.display(),
            framerate = ?self.framerate,
            profile = ?self.profile.id(),
            encoder = ?self.encoder,
            fd = self.fd,
            stream_len = self.streams.len(),
            streams = ?self.streams,
            record_desktop_audio = ?self.record_desktop_audio,
            record_microphone = ?self.record_microphone,
            select_area_data = ?self.select_area_data,
            area_crop = ?self.area_crop,
            pause_flash = ?self.pause_flash,
        );

        let pipeline = gst::Pipeline::new();

        let videosrc_bin = make_videosrc_bin(self.fd, &self.streams, self.framerate)
            .context("Failed to create videosrc bin")?;
        let filesink = gst::ElementFactory::make("filesink")
            .property(
                "location",
                self.file_path
                    .to_str()
                    .context("Could not convert file path to string")?,
            )
            .build()?;
        pipeline.add_many([videosrc_bin.upcast_ref(), &filesink])?;

        let video_out = if let Some(area) = self.area_crop {
            let videocrop = make_area_crop(area)?;
            pipeline.add(&videocrop)?;
            videosrc_bin.link(&videocrop)?;
            videocrop
        } else if let Some(ref data) = self.select_area_data {
            let crop = compute_crop(data);
            let videocrop = make_videocrop(&crop)?;
            let videoscale = gst::ElementFactory::make("videoscale").build()?;
            let capsfilter = gst::ElementFactory::make("capsfilter")
                .property("caps", videoscale_caps(cropped_size(&crop)))
                .build()?;
            pipeline.add_many([&videocrop, &videoscale, &capsfilter])?;
            gst::Element::link_many([
                videosrc_bin.upcast_ref(),
                &videocrop,
                &videoscale,
                &capsfilter,
            ])?;
            capsfilter
        } else {
            videosrc_bin.clone().upcast()
        };
        let videoenc_queue = link_gated_queue(
            &pipeline,
            &video_out,
            GatedBranch::Video,
            &self.pause_gate,
            self.pause_flash,
        )?;

        let audioenc_queue = if self.record_desktop_audio || self.record_microphone {
            debug_assert!(self.profile.supports_audio());

            let pulsesrcs = [
                self.record_desktop_audio
                    .then(|| make_pulsesrc(DeviceClass::Sink, "kooha-desktop-audio-src")),
                self.record_microphone
                    .then(|| make_pulsesrc(DeviceClass::Source, "kooha-microphone-src")),
            ];
            let audiosrc_bin = make_audiosrc_bin(
                &pulsesrcs
                    .into_iter()
                    .flatten()
                    .collect::<Result<Vec<_>>>()?,
            )
            .context("Failed to create audiosrc bin")?;
            pipeline.add(&audiosrc_bin)?;

            Some(link_gated_queue(
                &pipeline,
                audiosrc_bin.upcast_ref(),
                GatedBranch::Audio,
                &self.pause_gate,
                None,
            )?)
        } else {
            None
        };

        self.profile
            .attach_with(
                &pipeline,
                &videoenc_queue,
                audioenc_queue.as_ref(),
                &filesink,
                self.encoder,
            )
            .with_context(|| {
                format!(
                    "Failed to attach profile `{}` to pipeline",
                    self.profile.id()
                )
            })?;

        Ok(pipeline)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatedBranch {
    Video,
    Audio,
}

/// Links `src` through the pause gate into a new encoder queue, and returns
/// the queue. For video with `pause_flash`, the resume flash goes between
/// the gate and the queue.
pub fn link_gated_queue(
    pipeline: &gst::Pipeline,
    src: &gst::Element,
    branch: GatedBranch,
    gate: &PauseGate,
    pause_flash: Option<FlashColor>,
) -> Result<gst::Element> {
    let (gate_name, queue_name) = match branch {
        GatedBranch::Video => ("kooha-video-gate", "kooha-videoenc-queue"),
        GatedBranch::Audio => ("kooha-audio-gate", "kooha-audioenc-queue"),
    };

    let identity = gst::ElementFactory::make("identity")
        .name(gate_name)
        .build()?;
    let queue = gst::ElementFactory::make("queue")
        .name(queue_name)
        .build()?;
    if branch == GatedBranch::Video {
        // Room for a burst in the encoder without stalling the capture. The
        // byte cap bounds the memory of whole-monitor frames: 30 BGRx frames
        // of 2560x1600 are about 490 MB.
        queue.set_property("max-size-buffers", 30_u32);
        queue.set_property("max-size-time", 0_u64);
        queue.set_property("max-size-bytes", VIDEOENC_QUEUE_MAX_BYTES);
    }
    pipeline.add_many([&identity, &queue])?;
    src.link(&identity)?;

    // The probe goes on the gate's src pad, with nothing in between it and
    // the flash, so that both run in the same streaming thread.
    gate.attach(&identity.static_pad("src").unwrap());

    if let (GatedBranch::Video, Some(color)) = (branch, pause_flash) {
        let videobalance = gst::ElementFactory::make("videobalance")
            .name("kooha-pause-flash")
            .build()?;
        pipeline.add(&videobalance)?;
        gst::Element::link_many([&identity, &videobalance, &queue])?;
        gate.set_flash_element(&videobalance, color);
    } else {
        identity.link(&queue)?;
    }

    Ok(queue)
}

fn make_pipewiresrc(fd: RawFd, path: &str) -> Result<gst::Element> {
    let src = gst::ElementFactory::make("pipewiresrc")
        .property("fd", fd)
        .property("path", path)
        .property("do-timestamp", true)
        .property("provide-clock", false)
        .property("keepalive-time", 1000)
        .property("resend-last", true)
        .build()?;

    Ok(src)
}

fn make_videoflip() -> Result<gst::Element> {
    let videoflip = gst::ElementFactory::make("videoflip")
        .property_from_str("video-direction", "auto")
        .build()?;

    Ok(videoflip)
}

/// What `videocrop` cuts from each side of the frame, in stream pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub top: i32,
    pub left: i32,
    pub right: i32,
    pub bottom: i32,
    /// The size of the frame that enters `videocrop`, after `videoscale`
    /// made it even.
    pub frame: (i32, i32),
}

/// Computes the crop from the selection coordinates and the stream size.
pub fn compute_crop(data: &SelectAreaData) -> Crop {
    let SelectAreaData {
        selection,
        paintable_rect,
        stream_size,
    } = data;

    let (stream_width, stream_height) = stream_size;
    let scale_factor_h = *stream_width as f32 / paintable_rect.width();
    let scale_factor_v = *stream_height as f32 / paintable_rect.height();

    if scale_factor_h != scale_factor_v {
        tracing::warn!(
            scale_factor_h,
            scale_factor_v,
            "Scale factors of horizontal and vertical are unequal"
        );
    }

    // Both paintable and selection position are relative to the widget coordinates.
    // To get the absolute position and so correct crop values, subtract the paintable
    // rect's position from the selection rect.
    let old_selection_rect = selection.rect();
    let selection_rect_scaled = Rect::new(
        old_selection_rect.x() - paintable_rect.x(),
        old_selection_rect.y() - paintable_rect.y(),
        old_selection_rect.width(),
        old_selection_rect.height(),
    )
    .scale(scale_factor_h, scale_factor_v);

    let raw_top_crop = selection_rect_scaled.y();
    let raw_left_crop = selection_rect_scaled.x();
    let raw_right_crop =
        *stream_width as f32 - (selection_rect_scaled.width() + selection_rect_scaled.x());
    let raw_bottom_crop =
        *stream_height as f32 - (selection_rect_scaled.height() + selection_rect_scaled.y());

    tracing::debug!(raw_top_crop, raw_left_crop, raw_right_crop, raw_bottom_crop);

    // x264enc requires even resolution.
    let crop = Crop {
        top: round_to_even_f32(raw_top_crop).clamp(0, *stream_height),
        left: round_to_even_f32(raw_left_crop).clamp(0, *stream_width),
        right: round_to_even_f32(raw_right_crop).clamp(0, *stream_width),
        bottom: round_to_even_f32(raw_bottom_crop).clamp(0, *stream_height),
        frame: scaled_caps_size(*stream_size),
    };

    tracing::debug!(?crop);

    crop
}

/// The stream size rounded down to even; x264enc requires even resolution.
fn scaled_caps_size((width, height): (i32, i32)) -> (i32, i32) {
    (round_to_even(width), round_to_even(height))
}

/// The size the upstream selector records: the even frame minus the crop.
fn cropped_size(crop: &Crop) -> (i32, i32) {
    (
        crop.frame.0 - crop.left - crop.right,
        crop.frame.1 - crop.top - crop.bottom,
    )
}

/// The caps after `videoscale`: `size` rounded down to even.
fn videoscale_caps(size: (i32, i32)) -> gst::Caps {
    let (width, height) = scaled_caps_size(size);
    gst::Caps::builder("video/x-raw")
        .field("width", width)
        .field("height", height)
        .build()
}

/// Creates a videocrop element that cuts the given crop.
fn make_videocrop(crop: &Crop) -> Result<gst::Element> {
    let Crop {
        top,
        left,
        right,
        bottom,
        ..
    } = *crop;

    let crop = gst::ElementFactory::make("videocrop")
        .property("top", top)
        .property("left", left)
        .property("right", right)
        .property("bottom", bottom)
        .build()?;
    Ok(crop)
}

/// What to cut from the whole-monitor stream, in stream (physical) pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AreaCrop {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// The physical size of the monitor the area was marked on.
    pub expected_frame: (i32, i32),
}

/// How far, in pixels per axis, the real frame may be from the expected one
/// before the area counts as marked on another monitor.
const FRAME_TOLERANCE: i32 = 2;

/// The sides `videocrop` cuts so that `area` comes out of a frame of the
/// given size.
///
/// A frame up to `FRAME_TOLERANCE` pixels off the expected one is fine: an
/// area that would go past its edge moves inwards and keeps its size. A
/// frame further off, or smaller than the area, is an error, since the cut
/// would land elsewhere than where the area was marked.
fn area_crop_sides(area: &AreaCrop, frame: (i32, i32)) -> Result<Crop> {
    let (frame_w, frame_h) = frame;
    let (expected_w, expected_h) = area.expected_frame;

    if (frame_w - expected_w).abs() > FRAME_TOLERANCE
        || (frame_h - expected_h).abs() > FRAME_TOLERANCE
    {
        bail!(
            "The stream is {frame_w}x{frame_h}, but the area was marked on a \
             {expected_w}x{expected_h} monitor"
        );
    }
    if area.width > frame_w || area.height > frame_h {
        bail!(
            "The {}x{} area does not fit in the {frame_w}x{frame_h} stream",
            area.width,
            area.height
        );
    }

    let left = area.x.clamp(0, frame_w - area.width);
    let top = area.y.clamp(0, frame_h - area.height);
    if (left, top) != (area.x, area.y) {
        tracing::warn!(?area, ?frame, left, top, "Moved the area inside the stream");
    }

    Ok(Crop {
        top,
        left,
        right: frame_w - left - area.width,
        bottom: frame_h - top - area.height,
        frame,
    })
}

/// The width and height fields of video caps.
fn caps_size(caps: &gst::CapsRef) -> Option<(i32, i32)> {
    let structure = caps.structure(0)?;
    Some((
        structure.get::<i32>("width").ok()?,
        structure.get::<i32>("height").ok()?,
    ))
}

/// Creates a videocrop element that cuts `area`. The sides are set when the
/// caps arrive, from the real frame size; a frame that does not match the
/// area posts an error on the bus.
fn make_area_crop(area: AreaCrop) -> Result<gst::Element> {
    let videocrop = gst::ElementFactory::make("videocrop")
        .name("kooha-area-crop")
        .property("left", area.x)
        .property("top", area.y)
        .build()?;

    let weak = videocrop.downgrade();
    videocrop
        .static_pad("sink")
        .unwrap()
        .add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
            let Some(gst::EventView::Caps(event)) = info.event().map(|event| event.view()) else {
                return gst::PadProbeReturn::Ok;
            };
            let (Some(videocrop), Some(frame)) = (weak.upgrade(), caps_size(event.caps())) else {
                return gst::PadProbeReturn::Ok;
            };

            area_crop_sides(&area, frame).map_or_else(
                |error| {
                    gst::element_error!(videocrop, gst::StreamError::Format, ("{}", error));
                    gst::PadProbeReturn::Drop
                },
                |crop| {
                    tracing::debug!(?area, ?crop, "Cropping the area");
                    videocrop.set_property("left", crop.left);
                    videocrop.set_property("top", crop.top);
                    videocrop.set_property("right", crop.right);
                    videocrop.set_property("bottom", crop.bottom);
                    gst::PadProbeReturn::Ok
                },
            )
        })
        .expect("probes can be added to any pad");

    Ok(videocrop)
}

/// Counts the frames the capture queue drops, and logs the total when the
/// queue goes away.
#[derive(Debug, Default)]
struct DroppedFrames(Arc<AtomicU64>);

impl Drop for DroppedFrames {
    fn drop(&mut self) {
        let dropped = self.0.load(Ordering::Relaxed);
        tracing::info!(dropped, "Capture queue dropped frames");
    }
}

/// Creates the queue between the capture and `videorate`. It holds three
/// frames and, when full, drops the oldest instead of stalling `pipewiresrc`;
/// `videorate` then fills the gap. Also returns the count of dropped frames.
fn make_capture_queue() -> Result<(gst::Element, Arc<AtomicU64>)> {
    let queue = gst::ElementFactory::make("queue")
        .name("kooha-capture-queue")
        .property("max-size-buffers", 3_u32)
        .property("max-size-time", 0_u64)
        .property("max-size-bytes", 0_u32)
        .property_from_str("leaky", "downstream")
        .build()?;

    // A full leaky queue emits `overrun` and then drops one buffer.
    let dropped = DroppedFrames::default();
    let count = Arc::clone(&dropped.0);
    queue.connect("overrun", false, move |_| {
        dropped.0.fetch_add(1, Ordering::Relaxed);
        None
    });

    Ok((queue, count))
}

/// Creates a bin with a src pad for multiple pipewire streams.
///
/// Single stream:
///
/// pipewiresrc -> videoflip -> capture queue -> videorate
///
/// Multiple streams:
///
/// pipewiresrc1 -> videoflip -> |
///                              |
/// pipewiresrc2 -> videoflip -> | -> compositor -> capture queue -> videorate
///                              |
/// pipewiresrcn -> videoflip -> |
pub fn make_videosrc_bin(
    fd: RawFd,
    streams: &[Stream],
    framerate: gst::Fraction,
) -> Result<gst::Bin> {
    // TODO Create a bin that hotswaps compositor depending whether gl is supported or not.

    let bin = gst::Bin::builder().name("kooha-pipewiresrc-bin").build();

    let videorate = gst::ElementFactory::make("videorate")
        .property("skip-to-first", true)
        .build()?;
    let videorate_capsfilter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("framerate", framerate)
                .build(),
        )
        .build()?;
    let (capture_queue, _) = make_capture_queue()?;
    bin.add_many([&capture_queue, &videorate, &videorate_capsfilter])?;
    gst::Element::link_many([&capture_queue, &videorate, &videorate_capsfilter])?;

    match streams {
        [] => bail!("No streams provided"),
        [stream] => {
            let pipewiresrc = make_pipewiresrc(fd, &stream.node_id().to_string())?;
            let videoflip = make_videoflip()?;
            bin.add_many([&pipewiresrc, &videoflip])?;
            gst::Element::link_many([&pipewiresrc, &videoflip, &capture_queue])?;
        }
        streams => {
            let compositor = gst::ElementFactory::make("compositor").build()?;
            bin.add(&compositor)?;
            compositor.link(&capture_queue)?;

            let mut last_pos = 0;
            for stream in streams {
                let pipewiresrc = make_pipewiresrc(fd, &stream.node_id().to_string())?;
                let videoflip = make_videoflip()?;
                bin.add_many([&pipewiresrc, &videoflip])?;
                pipewiresrc.link(&videoflip)?;

                let compositor_sink_pad = compositor
                    .request_pad_simple("sink_%u")
                    .context("Failed to request sink_%u pad from compositor")?;
                compositor_sink_pad.set_property("xpos", last_pos);
                videoflip
                    .static_pad("src")
                    .unwrap()
                    .link(&compositor_sink_pad)?;

                let (stream_width, _) = stream.size().unwrap();
                last_pos += stream_width;
            }
        }
    }

    let src_pad = videorate_capsfilter.static_pad("src").unwrap();
    bin.add_pad(&gst::GhostPad::with_target(&src_pad)?)?;

    Ok(bin)
}

/// Creates a new audio src element with the given name.
///
/// If the class is already a source, it will return the device name as is,
/// otherwise, if it is a sink, it will append `.monitor` to the device name.
fn make_pulsesrc(class: DeviceClass, element_name: &str) -> Result<gst::Element> {
    let device = device::find_default(class)?;

    let pulsesrc = gst::ElementFactory::make("pulsesrc")
        .name(element_name)
        .property("provide-clock", false)
        .property("do-timestamp", true)
        .build()?;

    match class {
        DeviceClass::Sink => {
            let pulsesink = device.create_element(None)?;
            let device_name = pulsesink
                .property::<Option<String>>("device")
                .context("No device name")?;
            ensure!(!device_name.is_empty(), "Empty device name");

            let monitor_name = format!("{}.monitor", device_name);
            pulsesrc.set_property("device", &monitor_name);

            tracing::debug!("Found desktop audio with name `{}`", monitor_name);
        }
        DeviceClass::Source => {
            device.reconfigure_element(&pulsesrc)?;

            let device_name = pulsesrc
                .property::<Option<String>>("device")
                .context("No device name")?;
            ensure!(!device_name.is_empty(), "Empty device name");

            tracing::debug!("Found microphone with name `{}`", device_name);
        }
    }

    Ok(pulsesrc)
}

/// Creates a bin with a src pad for a pulse audio device
///
/// pulsesrc1 -> audiorate -> |
///                           |
/// pulsesrc2 -> audiorate -> | -> audiomixer
///                           |
/// pulsesrcn -> audiorate -> |
pub fn make_audiosrc_bin<'a>(
    pulsesrcs: impl IntoIterator<Item = &'a gst::Element>,
) -> Result<gst::Bin> {
    let bin = gst::Bin::builder().name("kooha-pulsesrc-bin").build();

    let caps = gst::Caps::builder_full()
        .structure(
            gst::Structure::builder("audio/x-raw")
                .field("rate", AUDIO_SAMPLE_RATE)
                .field("channels", 2)
                .build(),
        )
        .structure(
            gst::Structure::builder("audio/x-raw")
                .field("rate", AUDIO_SAMPLE_RATE)
                .field("channels", 1)
                .build(),
        )
        .build();

    let audiomixer = gst::ElementFactory::make("audiomixer")
        .property("latency", gst::ClockTime::from_seconds(2))
        .build()?;
    let audiomixer_capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()?;
    bin.add_many([&audiomixer, &audiomixer_capsfilter])?;
    audiomixer.link(&audiomixer_capsfilter)?;

    let src_pad = audiomixer_capsfilter.static_pad("src").unwrap();
    bin.add_pad(&gst::GhostPad::with_target(&src_pad)?)?;

    for pulsesrc in pulsesrcs {
        let audiorate = gst::ElementFactory::make("audiorate")
            .property("skip-to-first", true)
            .build()?;

        bin.add_many([pulsesrc, &audiorate])?;
        pulsesrc.link_filtered(&audiorate, &caps)?;
        audiorate.link_pads(None, &audiomixer, Some("sink_%u"))?;
    }

    Ok(bin)
}

fn round_to_even(number: i32) -> i32 {
    number / 2 * 2
}

fn round_to_even_f32(number: f32) -> i32 {
    (number / 2.0).round() as i32 * 2
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{area_selector::Selection, settings::test_utils::init_resources};

    use gtk::glib::{self, variant::StaticVariantType};
    // The module imports anyhow's `Ok`; these tests build std results.
    use std::result::Result::Ok;

    macro_rules! assert_even {
        ($number:expr) => {
            assert_eq!($number % 2, 0)
        };
    }

    #[test]
    fn odd_round_to_even() {
        assert_even!(round_to_even(5));
        assert_even!(round_to_even(101));
    }

    #[test]
    fn odd_round_to_even_f32() {
        assert_even!(round_to_even_f32(3.0));
        assert_even!(round_to_even_f32(99.0));
    }

    #[test]
    fn even_round_to_even() {
        assert_even!(round_to_even(50));
        assert_even!(round_to_even(4));
    }

    #[test]
    fn even_round_to_even_f32() {
        assert_even!(round_to_even_f32(300.0));
        assert_even!(round_to_even_f32(6.0));
    }

    #[test]
    fn float_round_to_even_f32() {
        assert_even!(round_to_even_f32(5.3));
        assert_even!(round_to_even_f32(2.9));
    }

    fn select_area_data(stream_size: (i32, i32)) -> SelectAreaData {
        SelectAreaData {
            selection: Selection::from_rect(110.0, 60.0, 400.0, 300.0),
            paintable_rect: Rect::new(10.0, 10.0, 960.0, 540.0),
            stream_size,
        }
    }

    #[test]
    fn crop_from_selection() {
        // Scale 2 on both axes: the selection minus the paintable origin is
        // (100, 50, 400, 300), which scaled is (200, 100, 800, 600).
        assert_eq!(
            compute_crop(&select_area_data((1920, 1080))),
            Crop {
                top: 100,
                left: 200,
                right: 920,
                bottom: 380,
                frame: (1920, 1080),
            }
        );
    }

    #[test]
    fn odd_stream_size_uses_even_frame() {
        gst::init().unwrap();

        let crop = compute_crop(&select_area_data((1921, 1081)));
        assert_eq!(crop.frame, (1920, 1080));

        // The frame is what `videoscale` hands to `videocrop`.
        let caps = videoscale_caps((1921, 1081));
        let structure = caps.structure(0).unwrap();
        assert_eq!(
            (
                structure.get::<i32>("width").unwrap(),
                structure.get::<i32>("height").unwrap()
            ),
            crop.frame
        );
    }

    /// Names of the elements downstream of `element`, following `src` pads.
    fn downstream_names(element: &gst::Element) -> Vec<String> {
        let mut names = Vec::new();
        let mut current = element.clone();
        while let Some(next) = current
            .static_pad("src")
            .and_then(|pad| pad.peer())
            .and_then(|peer| peer.parent_element())
        {
            names.push(next.name().to_string());
            current = next;
        }
        names
    }

    /// The pipeline goes back too: dropping it would unlink its elements.
    fn gated(
        branch: GatedBranch,
        pause_flash: bool,
    ) -> (gst::Pipeline, gst::Element, gst::Element) {
        gst::init().unwrap();

        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("fakesrc").build().unwrap();
        pipeline.add(&src).unwrap();
        let flash = pause_flash.then_some(FlashColor::White);
        let queue =
            link_gated_queue(&pipeline, &src, branch, &PauseGate::default(), flash).unwrap();
        (pipeline, src, queue)
    }

    #[test]
    fn video_goes_through_gate_and_flash() {
        let (_pipeline, src, queue) = gated(GatedBranch::Video, true);

        assert_eq!(queue.name(), "kooha-videoenc-queue");
        assert_eq!(
            downstream_names(&src),
            [
                "kooha-video-gate",
                "kooha-pause-flash",
                "kooha-videoenc-queue"
            ]
        );
    }

    #[test]
    fn flash_is_left_out_when_disabled() {
        let (_pipeline, src, _) = gated(GatedBranch::Video, false);

        assert_eq!(
            downstream_names(&src),
            ["kooha-video-gate", "kooha-videoenc-queue"]
        );
    }

    #[test]
    fn audio_goes_through_gate_without_flash() {
        let (_pipeline, src, queue) = gated(GatedBranch::Audio, true);

        assert_eq!(queue.name(), "kooha-audioenc-queue");
        assert_eq!(
            downstream_names(&src),
            ["kooha-audio-gate", "kooha-audioenc-queue"]
        );
    }

    fn factory_name(element: &gst::Element) -> String {
        element
            .factory()
            .map(|factory| factory.name().to_string())
            .unwrap_or_default()
    }

    /// Factory names of the elements downstream of `element`.
    fn downstream_factories(element: &gst::Element) -> Vec<String> {
        let mut names = Vec::new();
        let mut current = element.clone();
        while let Some(next) = current
            .static_pad("src")
            .and_then(|pad| pad.peer())
            .and_then(|peer| peer.parent_element())
        {
            names.push(factory_name(&next));
            current = next;
        }
        names
    }

    /// A whole recording pipeline for one 1920x1080 stream, not started.
    fn built_pipeline(configure: impl FnOnce(&mut PipelineBuilder)) -> gst::Pipeline {
        gst::init().unwrap();
        init_resources();

        let stream = glib::Variant::parse(
            Some(&Stream::static_variant_type()),
            "(uint32 63, {'size': <(1920, 1080)>})",
        )
        .unwrap()
        .get::<Stream>()
        .unwrap();
        let mut builder = PipelineBuilder::new(
            PathBuf::from("/tmp/kooha-pipeline-test-unused.mp4"),
            gst::Fraction::new(30, 1),
            Profile::from_id("mp4").unwrap().clone(),
            -1,
            vec![stream],
        );
        configure(&mut builder);
        builder.build().unwrap()
    }

    fn videosrc_bin(pipeline: &gst::Pipeline) -> gst::Element {
        pipeline.by_name("kooha-pipewiresrc-bin").unwrap()
    }

    const AREA: AreaCrop = AreaCrop {
        x: 100,
        y: 80,
        width: 1280,
        height: 720,
        expected_frame: (1680, 1050),
    };

    fn has_factory(pipeline: &gst::Pipeline, name: &str) -> bool {
        pipeline
            .iterate_recurse()
            .into_iter()
            .any(|element| factory_name(&element.unwrap()) == name)
    }

    #[test]
    fn the_cpu_encoder_is_the_default() {
        let pipeline = built_pipeline(|_| {});

        assert!(has_factory(&pipeline, "x264enc"));
    }

    #[test]
    fn the_chosen_encoder_is_attached() {
        gst::init().unwrap();
        for encoder in [H264Encoder::Intel, H264Encoder::Nvidia] {
            if !encoder.is_installed() {
                eprintln!("Skipped {encoder:?}: it is not installed");
                continue;
            }

            let pipeline = built_pipeline(|builder| {
                builder.encoder(encoder);
            });
            assert!(!has_factory(&pipeline, "x264enc"), "{encoder:?}");
            for element in encoder.required_elements() {
                assert!(has_factory(&pipeline, element), "{encoder:?} {element}");
            }
        }
    }

    fn stream_of(size: Option<(i32, i32)>) -> Stream {
        let text = match size {
            Some((width, height)) => format!("(uint32 63, {{'size': <({width}, {height})>}})"),
            None => "(uint32 63, @a{sv} {})".to_string(),
        };
        glib::Variant::parse(Some(&Stream::static_variant_type()), &text)
            .unwrap()
            .get::<Stream>()
            .unwrap()
    }

    fn builder_for(sizes: &[Option<(i32, i32)>]) -> PipelineBuilder {
        PipelineBuilder::new(
            PathBuf::from("/tmp/kooha-pipeline-test-unused.mp4"),
            gst::Fraction::new(30, 1),
            Profile::from_id("mp4").unwrap().clone(),
            -1,
            sizes.iter().copied().map(stream_of).collect(),
        )
    }

    #[test]
    fn output_size_of_an_area_is_the_area() {
        init_resources();

        let mut builder = builder_for(&[Some((1680, 1050))]);
        builder.area_crop(AREA);
        assert_eq!(builder.output_size(), Some((1280, 720)));
    }

    #[test]
    fn output_size_of_the_selector_is_the_scaled_crop() {
        init_resources();

        let mut builder = builder_for(&[Some((1921, 1081))]);
        builder.select_area_data(select_area_data((1921, 1081)));
        assert_eq!(builder.output_size(), Some((800, 600)));
    }

    #[test]
    fn output_size_of_whole_streams_is_side_by_side() {
        init_resources();

        assert_eq!(
            builder_for(&[Some((1920, 1080))]).output_size(),
            Some((1920, 1080))
        );
        assert_eq!(
            builder_for(&[Some((1920, 1080)), Some((2560, 1600))]).output_size(),
            Some((4480, 1600))
        );
        assert_eq!(builder_for(&[Some((1920, 1080)), None]).output_size(), None);
    }

    #[test]
    fn area_crop_needs_no_videoscale() {
        let pipeline = built_pipeline(|builder| {
            builder.area_crop(AREA);
        });

        let factories = pipeline
            .iterate_recurse()
            .into_iter()
            .map(|element| factory_name(&element.unwrap()))
            .collect::<Vec<_>>();
        assert!(!factories.iter().any(|name| name == "videoscale"));

        let downstream = downstream_factories(&videosrc_bin(&pipeline));
        assert_eq!(downstream[..2], ["videocrop", "identity"]);
    }

    #[test]
    fn selector_path_crops_before_scaling() {
        let pipeline = built_pipeline(|builder| {
            builder.select_area_data(select_area_data((1920, 1080)));
        });

        let downstream = downstream_factories(&videosrc_bin(&pipeline));
        assert_eq!(
            downstream[..4],
            ["videocrop", "videoscale", "capsfilter", "identity"]
        );
    }

    #[test]
    fn selector_path_scales_the_cropped_size_to_even() {
        // Stream 1921x1081 cropped by (top 100, left 200, right 920, bottom
        // 380) leaves 801x601, which `videoscale` makes 800x600.
        let pipeline = built_pipeline(|builder| {
            builder.select_area_data(select_area_data((1921, 1081)));
        });

        let capsfilter = videosrc_bin(&pipeline)
            .static_pad("src")
            .and_then(|pad| pad.peer())
            .and_then(|peer| peer.parent_element())
            .and_then(|videocrop| videocrop.static_pad("src"))
            .and_then(|pad| pad.peer())
            .and_then(|peer| peer.parent_element())
            .and_then(|videoscale| videoscale.static_pad("src"))
            .and_then(|pad| pad.peer())
            .and_then(|peer| peer.parent_element())
            .unwrap();
        let caps = capsfilter.property::<gst::Caps>("caps");
        let structure = caps.structure(0).unwrap();
        assert_eq!(
            (
                structure.get::<i32>("width").unwrap(),
                structure.get::<i32>("height").unwrap()
            ),
            (800, 600)
        );
    }

    /// Runs one `frame`-sized test buffer through the area crop. Returns the
    /// crop element and the caps that came out, or the bus error.
    fn run_area_crop(
        frame: (i32, i32),
        area: AreaCrop,
    ) -> (gst::Element, std::result::Result<(i32, i32), String>) {
        gst::init().unwrap();

        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 1)
            .build()
            .unwrap();
        let videocrop = make_area_crop(area).unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();
        pipeline.add_many([&src, &videocrop, &sink]).unwrap();
        src.link_filtered(
            &videocrop,
            &gst::Caps::builder("video/x-raw")
                .field("format", "BGRx")
                .field("width", frame.0)
                .field("height", frame.1)
                .build(),
        )
        .unwrap();
        videocrop.link(&sink).unwrap();

        pipeline.set_state(gst::State::Playing).unwrap();
        let message = pipeline
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(5),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("the pipeline ends within 5 s");
        let result = match message.view() {
            gst::MessageView::Error(error) => Err(error.error().to_string()),
            _ => {
                let caps = sink.static_pad("sink").unwrap().current_caps().unwrap();
                let structure = caps.structure(0).unwrap();
                Ok((
                    structure.get::<i32>("width").unwrap(),
                    structure.get::<i32>("height").unwrap(),
                ))
            }
        };
        pipeline.set_state(gst::State::Null).unwrap();
        (videocrop, result)
    }

    fn sides(videocrop: &gst::Element) -> [i32; 4] {
        ["left", "top", "right", "bottom"].map(|side| videocrop.property::<i32>(side))
    }

    #[test]
    fn area_crop_sets_right_and_bottom_from_caps() {
        let (videocrop, result) = run_area_crop((1680, 1050), AREA);

        assert_eq!(result, Ok((1280, 720)));
        assert_eq!(sides(&videocrop), [100, 80, 300, 250]);
    }

    /// An area against the bottom right corner of a 1680x1050 monitor.
    const CORNER: AreaCrop = AreaCrop {
        x: 400,
        y: 330,
        width: 1280,
        height: 720,
        expected_frame: (1680, 1050),
    };

    #[test]
    fn area_crop_shifts_inside_a_frame_one_pixel_smaller() {
        let (videocrop, result) = run_area_crop((1679, 1049), CORNER);

        assert_eq!(result, Ok((1280, 720)));
        assert_eq!(sides(&videocrop), [399, 329, 0, 0]);
    }

    #[test]
    fn area_crop_shifts_inside_a_frame_two_pixels_smaller() {
        let (videocrop, result) = run_area_crop((1678, 1048), CORNER);

        assert_eq!(result, Ok((1280, 720)));
        assert_eq!(sides(&videocrop), [398, 328, 0, 0]);
    }

    #[test]
    fn area_crop_fails_on_a_frame_of_another_size() {
        let (_, result) = run_area_crop((1677, 1050), CORNER);

        let error = result.unwrap_err();
        assert!(error.contains("1677x1050"), "{error}");
    }

    #[test]
    fn area_crop_fails_when_the_area_is_larger_than_the_frame() {
        let area = AreaCrop {
            x: 0,
            y: 0,
            width: 1680,
            height: 720,
            expected_frame: (1680, 1050),
        };
        let (_, result) = run_area_crop((1679, 1050), area);

        let error = result.unwrap_err();
        assert!(error.contains("1680x720"), "{error}");
    }

    #[test]
    fn area_crop_sides_within_the_frame_are_kept() {
        assert_eq!(
            area_crop_sides(&AREA, (1681, 1052)).unwrap(),
            Crop {
                top: 80,
                left: 100,
                right: 301,
                bottom: 252,
                frame: (1681, 1052),
            }
        );
    }

    fn queue_limits(queue: &gst::Element) -> (u32, u64, u32) {
        (
            queue.property::<u32>("max-size-buffers"),
            queue.property::<u64>("max-size-time"),
            queue.property::<u32>("max-size-bytes"),
        )
    }

    fn leaky_nick(queue: &gst::Element) -> String {
        let value = queue.property_value("leaky");
        glib::EnumValue::from_value(&value)
            .map(|(_, leaky)| leaky.nick().to_string())
            .unwrap()
    }

    fn peer_factory(element: &gst::Element, pad_name: &str) -> String {
        let peer = element
            .static_pad(pad_name)
            .and_then(|pad| pad.peer())
            .and_then(|peer| peer.parent_element())
            .unwrap();
        factory_name(&peer)
    }

    #[test]
    fn capture_queue_sits_between_videoflip_and_videorate() {
        let pipeline = built_pipeline(|_| {});

        let queue = pipeline.by_name("kooha-capture-queue").unwrap();
        assert_eq!(queue_limits(&queue), (3, 0, 0));
        assert_eq!(leaky_nick(&queue), "downstream");
        assert_eq!(peer_factory(&queue, "sink"), "videoflip");
        assert_eq!(peer_factory(&queue, "src"), "videorate");
    }

    #[test]
    fn videoenc_queue_holds_thirty_buffers_up_to_256_mib() {
        let pipeline = built_pipeline(|_| {});

        let queue = pipeline.by_name("kooha-videoenc-queue").unwrap();
        assert_eq!(queue_limits(&queue), (30, 0, 256 * 1024 * 1024));
        assert_eq!(leaky_nick(&queue), "no");
    }

    #[test]
    fn capture_queue_counts_dropped_frames() {
        gst::init().unwrap();

        let (queue, dropped) = make_capture_queue().unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();
        let pipeline = gst::Pipeline::new();
        pipeline.add_many([&queue, &sink]).unwrap();
        queue.link(&sink).unwrap();
        pipeline.set_state(gst::State::Paused).unwrap();

        let src = gst::Pad::builder(gst::PadDirection::Src).build();
        src.set_active(true).unwrap();
        src.link(&queue.static_pad("sink").unwrap()).unwrap();
        src.push_event(gst::event::StreamStart::new("capture-queue-test"));
        src.push_event(gst::event::Caps::new(&gst::Caps::new_empty_simple(
            "video/x-raw",
        )));
        src.push_event(gst::event::Segment::new(&gst::FormattedSegment::<
            gst::ClockTime,
        >::new()));

        // The first buffer reaches the sink, which holds it while paused, so
        // the queue's thread stays blocked from then on.
        src.push(gst::Buffer::new()).unwrap();
        let start = std::time::Instant::now();
        while queue.property::<u32>("current-level-buffers") > 0 {
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            std::thread::yield_now();
        }

        // Three fill the queue; each of the other seven drops the oldest.
        for _ in 0..10 {
            src.push(gst::Buffer::new()).unwrap();
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 7);
        assert_eq!(queue.property::<u32>("current-level-buffers"), 3);

        pipeline.set_state(gst::State::Null).unwrap();
    }
}
