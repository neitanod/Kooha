use std::{
    ops::ControlFlow,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use gst::prelude::*;

/// Pause intervals in pipeline running time, and the mapping from input
/// timestamps to output timestamps without the paused spans.
#[derive(Debug, Default, Clone)]
pub struct PauseTimeline {
    /// Closed pauses as `[start, end)`, in order.
    closed: Vec<(gst::ClockTime, gst::ClockTime)>,
    open: Option<gst::ClockTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapped {
    Drop,
    Keep(gst::ClockTime),
}

impl PauseTimeline {
    /// Starts a pause at `at`. A second pause before resuming is ignored.
    pub fn pause(&mut self, at: gst::ClockTime) {
        if self.open.is_none() {
            self.open = Some(at);
        }
    }

    /// Ends the open pause at `at`. Without an open pause, it is ignored.
    pub fn resume(&mut self, at: gst::ClockTime) {
        if let Some(start) = self.open.take() {
            self.closed.push((start, at.max(start)));
        }
    }

    /// Drop if `ts` falls inside a pause (open or closed); otherwise
    /// subtract the length of every closed pause that ended at or before `ts`.
    pub fn map(&self, ts: gst::ClockTime) -> Mapped {
        if self.open.is_some_and(|start| ts >= start) {
            return Mapped::Drop;
        }

        let mut paused = gst::ClockTime::ZERO;
        for &(start, end) in &self.closed {
            if ts < start {
                break;
            }
            if ts < end {
                return Mapped::Drop;
            }
            paused += end - start;
        }

        Mapped::Keep(ts - paused)
    }

    /// Recorded time up to `now`, excluding pauses.
    pub fn recorded_duration(&self, now: gst::ClockTime) -> gst::ClockTime {
        let paused = self
            .closed
            .iter()
            .chain(self.open.map(|start| (start, now)).as_ref())
            .map(|&(start, end)| end.min(now).saturating_sub(start))
            .sum();

        now.saturating_sub(paused)
    }

    /// Output timestamp where the most recent resume lands, if any.
    pub fn last_resume_output(&self) -> Option<gst::ClockTime> {
        let &(_, last_end) = self.closed.last()?;

        Some(self.recorded_duration(last_end))
    }
}

/// How long the resume flash lasts, in output time.
const FLASH_DURATION: gst::ClockTime = gst::ClockTime::from_mseconds(150);

/// Shared between the GTK thread (pause/resume) and streaming threads (probes).
#[derive(Debug, Clone, Default)]
pub struct PauseGate(Arc<Mutex<GateState>>);

#[derive(Debug, Default)]
struct GateState {
    timeline: PauseTimeline,
    /// Set only when `pause-flash` is enabled.
    flash: Option<FlashState>,
}

#[derive(Debug)]
struct FlashState {
    videobalance: gst::Element,
    is_white: bool,
}

impl PauseGate {
    pub fn pause(&self, running_time: gst::ClockTime) {
        self.state().timeline.pause(running_time);
    }

    pub fn resume(&self, running_time: gst::ClockTime) {
        self.state().timeline.resume(running_time);
    }

    pub fn recorded_duration(&self, running_time: gst::ClockTime) -> gst::ClockTime {
        self.state().timeline.recorded_duration(running_time)
    }

    /// Installs a buffer probe on `pad` that drops or re-stamps PTS and DTS.
    ///
    /// Events pass untouched, so EOS goes through while paused.
    pub fn attach(&self, pad: &gst::Pad) {
        // The probe compares running times, so it needs the pad's segment.
        let segment = Arc::new(Mutex::new(gst::FormattedSegment::<gst::ClockTime>::new()));

        let segment_clone = Arc::clone(&segment);
        pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
            if let Some(gst::EventView::Segment(event)) = info.event().map(|event| event.view())
                && let Ok(new_segment) = event.segment().clone().downcast::<gst::ClockTime>()
            {
                *lock(&segment_clone) = new_segment;
            }
            gst::PadProbeReturn::Ok
        })
        .expect("probes can be added to any pad");

        let gate = self.clone();
        pad.add_probe(
            gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
            move |pad, info| {
                let segment = lock(&segment).clone();
                let mut state = gate.state();

                // Output running time of the last buffer that goes through.
                let mut last_kept = None;
                if let Some(buffer) = info.buffer_mut() {
                    match gate_buffer(&state.timeline, &segment, buffer) {
                        Some(Mapped::Drop) => return gst::PadProbeReturn::Drop,
                        Some(Mapped::Keep(output)) => last_kept = Some(output),
                        None => {}
                    }
                } else if let Some(list) = info.buffer_list_mut() {
                    let list = list.make_mut();
                    list.foreach_mut(|mut buffer, _| {
                        match gate_buffer(&state.timeline, &segment, &mut buffer) {
                            Some(Mapped::Drop) => return ControlFlow::Continue(None),
                            Some(Mapped::Keep(output)) => last_kept = Some(output),
                            None => {}
                        }
                        ControlFlow::Continue(Some(buffer))
                    });
                    if list.is_empty() {
                        return gst::PadProbeReturn::Drop;
                    }
                }

                let flash_change = last_kept.and_then(|output| state.flash_change(pad, output));
                drop(state);

                // The videobalance runs right after this pad, in this same
                // thread, so the change applies to this buffer.
                if let Some((videobalance, is_white)) = flash_change {
                    set_white(&videobalance, is_white);
                }

                gst::PadProbeReturn::Ok
            },
        )
        .expect("probes can be added to any pad");
    }

    /// Enables the resume flash driven by `videobalance`, which must be the
    /// element right after the gated video pad.
    pub fn set_flash_element(&self, videobalance: &gst::Element) {
        self.state().flash = Some(FlashState {
            videobalance: videobalance.clone(),
            is_white: false,
        });
    }

    fn state(&self) -> MutexGuard<'_, GateState> {
        lock(&self.0)
    }
}

impl GateState {
    /// The flash change a buffer at `output` on `pad` needs, if any.
    fn flash_change(
        &mut self,
        pad: &gst::Pad,
        output: gst::ClockTime,
    ) -> Option<(gst::Element, bool)> {
        let Self { timeline, flash } = self;
        let flash = flash.as_mut()?;

        let feeds_videobalance = pad
            .peer()
            .and_then(|peer| peer.parent_element())
            .is_some_and(|element| element == flash.videobalance);
        if !feeds_videobalance {
            return None;
        }

        let is_white = timeline
            .last_resume_output()
            .is_some_and(|resume| output >= resume && output < resume + FLASH_DURATION);
        if is_white == flash.is_white {
            return None;
        }

        flash.is_white = is_white;
        Some((flash.videobalance.clone(), is_white))
    }
}

fn set_white(videobalance: &gst::Element, is_white: bool) {
    let (saturation, brightness) = if is_white { (0.0, 1.0) } else { (1.0, 0.0) };
    videobalance.set_property("saturation", saturation);
    videobalance.set_property("brightness", brightness);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Drops `buffer` if it falls inside a pause, comparing in running time,
/// or shifts its PTS and DTS back by the pauses before it. Returns `None`
/// for a buffer without a PTS or outside `segment`, which passes unchanged.
fn gate_buffer(
    timeline: &PauseTimeline,
    segment: &gst::FormattedSegment<gst::ClockTime>,
    buffer: &mut gst::Buffer,
) -> Option<Mapped> {
    let pts = buffer.pts()?;
    let running_time = segment.to_running_time(pts)?;

    let mapped = timeline.map(running_time);
    if let Mapped::Keep(output_running_time) = mapped
        && let Some(new_pts) = segment.position_from_running_time(output_running_time)
        && new_pts != pts
    {
        let shift = pts.saturating_sub(new_pts);
        let buffer = buffer.make_mut();
        buffer.set_pts(new_pts);
        if let Some(dts) = buffer.dts() {
            buffer.set_dts(dts.saturating_sub(shift));
        }
    }

    Some(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn secs(secs: u64) -> gst::ClockTime {
        gst::ClockTime::from_seconds(secs)
    }

    #[test]
    fn no_pauses_is_identity() {
        let timeline = PauseTimeline::default();

        assert_eq!(timeline.map(secs(5)), Mapped::Keep(secs(5)));
    }

    #[test]
    fn inside_open_pause_drops() {
        let mut timeline = PauseTimeline::default();
        timeline.pause(secs(2));

        assert_eq!(timeline.map(secs(3)), Mapped::Drop);
        assert_eq!(timeline.map(secs(1)), Mapped::Keep(secs(1)));
    }

    #[test]
    fn after_resume_shifts() {
        let mut timeline = PauseTimeline::default();
        timeline.pause(secs(2));
        timeline.resume(secs(5));

        assert_eq!(timeline.map(secs(6)), Mapped::Keep(secs(3)));
        // Arrived late, but it falls inside the pause.
        assert_eq!(timeline.map(secs(4)), Mapped::Drop);
        assert_eq!(timeline.map(secs(5)), Mapped::Keep(secs(2)));
    }

    fn two_pauses() -> PauseTimeline {
        let mut timeline = PauseTimeline::default();
        timeline.pause(secs(2));
        timeline.resume(secs(5));
        timeline.pause(secs(7));
        timeline.resume(secs(8));
        timeline
    }

    #[test]
    fn two_pauses_accumulate() {
        let timeline = two_pauses();

        assert_eq!(timeline.map(secs(10)), Mapped::Keep(secs(6)));
        assert_eq!(timeline.map(secs(6)), Mapped::Keep(secs(3)));
        assert_eq!(timeline.map(secs(7)), Mapped::Drop);
    }

    #[test]
    fn recorded_duration_excludes_pauses() {
        let mut timeline = two_pauses();
        assert_eq!(timeline.recorded_duration(secs(10)), secs(6));

        timeline.pause(secs(9));
        assert_eq!(timeline.recorded_duration(secs(12)), secs(5));
    }

    #[test]
    fn resume_without_pause_is_ignored() {
        let mut timeline = PauseTimeline::default();
        timeline.resume(secs(3));

        assert_eq!(timeline.map(secs(5)), Mapped::Keep(secs(5)));
        assert_eq!(timeline.last_resume_output(), None);
    }

    #[test]
    fn double_pause_keeps_first_start() {
        let mut timeline = PauseTimeline::default();
        timeline.pause(secs(2));
        timeline.pause(secs(4));
        timeline.resume(secs(5));

        assert_eq!(timeline.map(secs(3)), Mapped::Drop);
        assert_eq!(timeline.map(secs(6)), Mapped::Keep(secs(3)));
    }

    #[test]
    fn last_resume_output() {
        let mut timeline = PauseTimeline::default();
        timeline.pause(secs(2));
        timeline.resume(secs(5));

        assert_eq!(timeline.last_resume_output(), Some(secs(2)));
        assert_eq!(two_pauses().last_resume_output(), Some(secs(4)));
    }

    // Integration tests with live GStreamer pipelines.

    const FRAME: gst::ClockTime = gst::ClockTime::from_nseconds(33_333_333);
    const STEP: Duration = Duration::from_millis(300);

    /// A pipeline whose `gate*` identities share one `PauseGate`, and the PTS
    /// that reach each `sink*` fakesink.
    struct Harness {
        pipeline: gst::Pipeline,
        gate: PauseGate,
        pts: Vec<Arc<Mutex<Vec<gst::ClockTime>>>>,
    }

    impl Harness {
        fn new(description: &str) -> Self {
            gst::init().unwrap();

            let pipeline = gst::parse::launch(description)
                .unwrap()
                .downcast::<gst::Pipeline>()
                .unwrap();
            let gate = PauseGate::default();
            let mut pts = Vec::new();

            for element in pipeline.iterate_elements().into_iter().map(Result::unwrap) {
                let name = element.name();
                if name.starts_with("gate") {
                    gate.attach(&element.static_pad("src").unwrap());
                } else if name.starts_with("sink") {
                    pts.push(collect_pts(&element));
                }
            }

            Self {
                pipeline,
                gate,
                pts,
            }
        }

        fn play(&self) {
            self.pipeline.set_state(gst::State::Playing).unwrap();
            let (res, state, _) = self.pipeline.state(gst::ClockTime::from_seconds(5));
            res.unwrap();
            assert_eq!(state, gst::State::Playing);
        }

        fn running_time(&self) -> gst::ClockTime {
            self.pipeline.current_running_time().unwrap()
        }

        /// Sends EOS and returns how long it took to reach the bus.
        fn stop(&self) -> Duration {
            let start = Instant::now();
            self.pipeline.send_event(gst::event::Eos::new());

            let message = self
                .pipeline
                .bus()
                .unwrap()
                .timed_pop_filtered(
                    gst::ClockTime::from_seconds(5),
                    &[gst::MessageType::Eos, gst::MessageType::Error],
                )
                .expect("no EOS within 5 s");
            let elapsed = start.elapsed();
            assert_eq!(message.type_(), gst::MessageType::Eos, "{message:?}");

            self.pipeline.set_state(gst::State::Null).unwrap();
            elapsed
        }

        fn pts(&self, index: usize) -> Vec<gst::ClockTime> {
            self.pts[index].lock().unwrap().clone()
        }

        /// Records, pauses and resumes for `STEP` each, then stops.
        /// Returns the running times of the pause, the resume and the stop.
        fn record_pause_resume(&self) -> (gst::ClockTime, gst::ClockTime, gst::ClockTime) {
            self.play();
            thread::sleep(STEP);
            let rt_pause = self.running_time();
            self.gate.pause(rt_pause);
            thread::sleep(STEP);
            let rt_resume = self.running_time();
            self.gate.resume(rt_resume);
            thread::sleep(STEP);
            let rt_final = self.running_time();
            self.stop();
            (rt_pause, rt_resume, rt_final)
        }
    }

    fn collect_pts(sink: &gst::Element) -> Arc<Mutex<Vec<gst::ClockTime>>> {
        let pts = Arc::new(Mutex::new(Vec::new()));
        let pts_clone = Arc::clone(&pts);
        sink.static_pad("sink")
            .unwrap()
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(pts) = info.buffer().and_then(|buffer| buffer.pts()) {
                    pts_clone.lock().unwrap().push(pts);
                }
                gst::PadProbeReturn::Ok
            })
            .unwrap();
        pts
    }

    /// Criterion C2, plus the paused span missing from the end.
    #[track_caller]
    fn assert_no_gap(
        pts: &[gst::ClockTime],
        (rt_pause, rt_resume, rt_final): (gst::ClockTime, gst::ClockTime, gst::ClockTime),
    ) {
        assert!(pts.len() > 10, "{pts:?}");
        for pair in pts.windows(2) {
            assert!(pair[0] < pair[1], "not increasing: {pair:?}");
            assert!(pair[1] - pair[0] <= 2 * FRAME, "gap: {pair:?}");
        }

        let last = *pts.last().unwrap();
        let expected_max = rt_final - (rt_resume - rt_pause) + FRAME;
        assert!(last <= expected_max, "last {last} > {expected_max}");
    }

    #[test]
    fn video_has_no_gap() {
        let harness = Harness::new(
            "videotestsrc is-live=true ! video/x-raw,framerate=30/1 ! identity name=gate ! fakesink name=sink sync=false",
        );

        let times = harness.record_pause_resume();

        assert_no_gap(&harness.pts(0), times);
    }

    #[test]
    fn audio_has_no_gap() {
        let harness = Harness::new(
            "audiotestsrc is-live=true ! audio/x-raw,rate=48000 ! identity name=gate ! fakesink name=sink sync=false",
        );

        let times = harness.record_pause_resume();

        assert_no_gap(&harness.pts(0), times);
    }

    const AV_PIPELINE: &str = "videotestsrc is-live=true ! video/x-raw,framerate=30/1 ! identity name=gate-video ! fakesink name=sink-video sync=false \
         audiotestsrc is-live=true ! audio/x-raw,rate=48000 ! identity name=gate-audio ! fakesink name=sink-audio sync=false";

    #[test]
    fn video_and_audio_jump_together() {
        let harness = Harness::new(AV_PIPELINE);

        let (rt_pause, rt_resume, rt_final) = harness.record_pause_resume();

        let (video, audio) = (harness.pts(0), harness.pts(1));
        let tolerance = gst::ClockTime::from_mseconds(50);
        let first_after = |pts: &[gst::ClockTime]| {
            *pts.iter()
                .find(|&&pts| pts >= rt_pause)
                .expect("no buffer after resuming")
        };
        let (video_first, audio_first) = (first_after(&video), first_after(&audio));
        assert!(
            video_first.absdiff(audio_first) < tolerance,
            "{video_first} vs {audio_first}"
        );
        let (video_last, audio_last) = (*video.last().unwrap(), *audio.last().unwrap());
        assert!(
            video_last.absdiff(audio_last) < tolerance,
            "{video_last} vs {audio_last}"
        );
        assert_no_gap(&video, (rt_pause, rt_resume, rt_final));
        assert_no_gap(&audio, (rt_pause, rt_resume, rt_final));
    }

    #[test]
    fn eos_while_paused() {
        let harness = Harness::new(AV_PIPELINE);

        harness.play();
        thread::sleep(STEP);
        let rt_pause = harness.running_time();
        harness.gate.pause(rt_pause);
        thread::sleep(STEP);
        let eos_delay = harness.stop();

        assert!(eos_delay < Duration::from_secs(2), "{eos_delay:?}");
        for pts in [harness.pts(0), harness.pts(1)] {
            assert!(!pts.is_empty());
            assert!(pts.iter().all(|&pts| pts < rt_pause), "{pts:?}");
        }
    }

    #[test]
    fn segment_not_starting_at_zero() {
        gst::init().unwrap();

        let mut timeline = PauseTimeline::default();
        timeline.pause(secs(2));
        timeline.resume(secs(5));

        for start in [secs(0), secs(10)] {
            let mut segment = gst::FormattedSegment::<gst::ClockTime>::new();
            segment.set_start(start);

            let mut paused = gst::Buffer::new();
            paused.get_mut().unwrap().set_pts(start + secs(3));
            assert_eq!(
                gate_buffer(&timeline, &segment, &mut paused),
                Some(Mapped::Drop)
            );

            let mut after = gst::Buffer::new();
            {
                let after = after.get_mut().unwrap();
                after.set_pts(start + secs(6));
                after.set_dts(start + secs(6) - FRAME);
            }
            assert_eq!(
                gate_buffer(&timeline, &segment, &mut after),
                Some(Mapped::Keep(secs(3)))
            );
            assert_eq!(after.pts(), Some(start + secs(3)));
            assert_eq!(after.dts(), Some(start + secs(3) - FRAME));

            let mut no_pts = gst::Buffer::new();
            assert_eq!(gate_buffer(&timeline, &segment, &mut no_pts), None);
        }
    }

    fn is_white(pixel: [u8; 3]) -> bool {
        pixel.iter().all(|&channel| channel >= 0xF0)
    }

    fn is_black(pixel: [u8; 3]) -> bool {
        pixel.iter().all(|&channel| channel <= 0x10)
    }

    /// Criterion C3.
    #[test]
    fn resume_flash_is_white() {
        let harness = Harness::new(
            "videotestsrc is-live=true pattern=black ! video/x-raw,format=BGRx,framerate=30/1 ! identity name=gate ! videobalance name=flash ! fakesink name=sink sync=false",
        );
        harness
            .gate
            .set_flash_element(&harness.pipeline.by_name("flash").unwrap());

        let frames = Arc::new(Mutex::new(Vec::new()));
        let frames_clone = Arc::clone(&frames);
        harness
            .pipeline
            .by_name("sink")
            .unwrap()
            .static_pad("sink")
            .unwrap()
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                let buffer = info.buffer().unwrap();
                let map = buffer.map_readable().unwrap();
                // BGRx
                let pixel = [map[0], map[1], map[2]];
                frames_clone
                    .lock()
                    .unwrap()
                    .push((buffer.pts().unwrap(), pixel));
                gst::PadProbeReturn::Ok
            })
            .unwrap();

        harness.record_pause_resume();

        let resume_out = harness.gate.state().timeline.last_resume_output().unwrap();
        let window_end = resume_out + FLASH_DURATION;
        let frames = frames.lock().unwrap().clone();
        let in_window = frames
            .iter()
            .filter(|(pts, _)| (resume_out..window_end).contains(pts))
            .collect::<Vec<_>>();
        assert!(in_window.len() >= 3, "{frames:?}");
        assert!(
            in_window.iter().all(|(_, pixel)| is_white(*pixel)),
            "{in_window:?}"
        );
        let (_, first_after) = frames
            .iter()
            .find(|(pts, _)| *pts >= window_end)
            .expect("no frame after the flash");
        assert!(is_black(*first_after), "{first_after:?}");
        assert!(
            frames
                .iter()
                .filter(|(pts, _)| *pts < resume_out)
                .all(|(_, pixel)| is_black(*pixel))
        );
    }

    /// How a `file_duration` recording ends.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Ending {
        /// Resumes and records `RECORD` more, then stops.
        Resume,
        /// Stops while still paused.
        StopWhilePaused,
    }

    const RECORD: Duration = Duration::from_secs(3);

    /// Whether the elements of the profile `profile_id` are installed.
    fn profile_is_available(profile_id: &str) -> bool {
        use crate::{profile::Profile, settings::test_utils::init_resources};

        init_resources();
        gst::init().unwrap();

        Profile::from_id(profile_id).unwrap().is_available()
    }

    /// Criterion C1: records `RECORD` seconds, pauses for `pause`, ends as
    /// `ending` says, and returns the duration of the file, with the
    /// encoders and muxers of the profile `profile_id`. The audio goes
    /// through the `audiomixer` of the recordings, with its 2 s latency
    /// (codex code r1 #6, fable code r1 #9). None, and a note, when an
    /// element of the profile is missing.
    fn file_duration(profile_id: &str, pause: Duration, ending: Ending) -> Option<gst::ClockTime> {
        use crate::{
            pipeline::{GatedBranch, link_gated_queue, make_audiosrc_bin},
            profile::Profile,
        };

        if !profile_is_available(profile_id) {
            eprintln!("Skipped: an element of {profile_id} is not installed");
            return None;
        }

        let profile = Profile::from_id(profile_id).unwrap();
        let path = std::env::temp_dir().join(format!(
            "kooha-test-file-duration-{}-{profile_id}-{}-{:?}.{}",
            std::process::id(),
            pause.as_secs(),
            ending,
            profile.file_extension()
        ));

        let pipeline = gst::Pipeline::new();
        let videosrc = gst::parse::bin_from_description(
            "videotestsrc is-live=true ! capsfilter caps=video/x-raw,framerate=30/1",
            true,
        )
        .unwrap();
        // Stands in for `pulsesrc`, which `make_audiosrc_bin` mixes.
        let audiotestsrc = gst::ElementFactory::make("audiotestsrc")
            .property("is-live", true)
            .build()
            .unwrap();
        let audiosrc = make_audiosrc_bin([&audiotestsrc]).unwrap();
        let filesink = gst::ElementFactory::make("filesink")
            .property("location", path.to_str().unwrap())
            .build()
            .unwrap();
        pipeline
            .add_many([videosrc.upcast_ref(), audiosrc.upcast_ref(), &filesink])
            .unwrap();

        let gate = PauseGate::default();
        let video_queue = link_gated_queue(
            &pipeline,
            videosrc.upcast_ref(),
            GatedBranch::Video,
            &gate,
            false,
        )
        .unwrap();
        let audio_queue = link_gated_queue(
            &pipeline,
            audiosrc.upcast_ref(),
            GatedBranch::Audio,
            &gate,
            false,
        )
        .unwrap();
        profile
            .attach(&pipeline, &video_queue, Some(&audio_queue), &filesink)
            .unwrap();

        pipeline.set_state(gst::State::Playing).unwrap();
        thread::sleep(RECORD);
        gate.pause(pipeline.current_running_time().unwrap());
        thread::sleep(pause);
        if ending == Ending::Resume {
            gate.resume(pipeline.current_running_time().unwrap());
            thread::sleep(RECORD);
        }

        pipeline.send_event(gst::event::Eos::new());
        let message = pipeline
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("no EOS within 10 s");
        assert_eq!(message.type_(), gst::MessageType::Eos, "{message:?}");
        pipeline.set_state(gst::State::Null).unwrap();

        let uri = gst::glib::filename_to_uri(&path, None).unwrap();
        let info = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(10))
            .unwrap()
            .discover_uri(&uri)
            .unwrap();
        std::fs::remove_file(&path).unwrap();

        // Readable, with its audio and video.
        assert_eq!(info.video_streams().len(), 1, "{profile_id}");
        assert_eq!(info.audio_streams().len(), 1, "{profile_id}");

        Some(info.duration().expect("the file has no duration"))
    }

    #[track_caller]
    fn assert_lasts(duration: Option<gst::ClockTime>, expected: gst::ClockTime) {
        let Some(duration) = duration else {
            return;
        };
        eprintln!("file duration: {duration}");
        assert!(
            duration.absdiff(expected) <= gst::ClockTime::from_mseconds(300),
            "{duration}, expected {expected}"
        );
    }

    #[test]
    fn file_duration_webm_vp8() {
        assert_lasts(
            file_duration("webm-vp8", Duration::from_secs(2), Ending::Resume),
            secs(6),
        );
    }

    /// The pause of criterion C1, as long as the latency of the
    /// `audiomixer`.
    #[test]
    fn file_duration_mp4() {
        assert_lasts(
            file_duration("mp4", Duration::from_secs(2), Ending::Resume),
            secs(6),
        );
    }

    /// Longer than the 2 s latency of the `audiomixer`, as `mp4mux` works
    /// with clock deadlines in a live pipeline.
    #[test]
    fn file_duration_mp4_long_pause() {
        assert_lasts(
            file_duration("mp4", Duration::from_secs(4), Ending::Resume),
            secs(6),
        );
    }

    #[test]
    fn file_duration_matroska_h264() {
        assert_lasts(
            file_duration("matroska-h264", Duration::from_secs(2), Ending::Resume),
            secs(6),
        );
    }

    #[test]
    fn file_duration_matroska_h264_long_pause() {
        assert_lasts(
            file_duration("matroska-h264", Duration::from_secs(4), Ending::Resume),
            secs(6),
        );
    }

    /// Stopping while paused finishes a readable file with what was
    /// recorded before the pause.
    #[test]
    fn file_duration_stop_while_paused() {
        assert_lasts(
            file_duration("mp4", Duration::from_secs(2), Ending::StopWhilePaused),
            secs(3),
        );
    }
}
