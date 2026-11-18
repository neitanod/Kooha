//! The H.264 encoders a recording can use, and the probe that tells whether
//! one works before recording with it (ADR 0006).

use std::{
    collections::HashMap,
    future::Future,
    hash::Hash,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use gst::prelude::*;
use gtk::{gio, glib};

use crate::{profile, settings::VideoEncoder};

/// How long a probe may take. A sleeping NVIDIA card took several seconds to
/// start in docs/perf/2026-11-18-capture-bench.md.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The encoders Automatic tries, in order, with the CPU always last.
///
/// Provisional, from the synthetic-source run of
/// docs/perf/2026-11-18-capture-bench.md: NVIDIA kept 60 FPS with the least
/// CPU, and Intel kept 60 FPS only with the CPU conversion to NV12. The run
/// with the KDE portal confirms or changes it.
pub const AUTO_ORDER: [H264Encoder; 3] = [H264Encoder::Nvidia, H264Encoder::Intel, H264Encoder::Cpu];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum H264Encoder {
    Intel,
    Nvidia,
    Cpu,
}

impl H264Encoder {
    /// The name shown to the user. Brand names are not translated.
    pub fn name(self) -> &'static str {
        match self {
            Self::Intel => "Intel",
            Self::Nvidia => "NVIDIA",
            Self::Cpu => "CPU",
        }
    }

    /// The `gst-launch` chain from raw video to `video/x-h264`, before
    /// `queue ! h264parse`. It has `${N_THREADS}`: use it through
    /// `profile::expand`.
    pub fn chain(self) -> &'static str {
        match self {
            // Converting to NV12 in the CPU first keeps `vapostproc` from
            // pulling its VA memory pool up to the source, which did not hold
            // 60 FPS (`intel` against `intel-convert` in the capture bench).
            Self::Intel => concat!(
                "videoconvert chroma-mode=none dither=none matrix-mode=output-only n-threads=${N_THREADS} ! ",
                "video/x-raw,format=NV12 ! ",
                "vapostproc ! ",
                "video/x-raw(memory:VAMemory),format=NV12 ! ",
                "vah264lpenc",
            ),
            Self::Nvidia => concat!(
                "cudaupload ! ",
                "cudaconvert ! ",
                "video/x-raw(memory:CUDAMemory),format=NV12 ! ",
                "nvh264enc",
            ),
            Self::Cpu => concat!(
                "videoconvert chroma-mode=none dither=none matrix-mode=output-only n-threads=${N_THREADS} ! ",
                "x264enc qp-max=17 speed-preset=ultrafast threads=${N_THREADS} ! ",
                "capsfilter caps=video/x-h264,profile=baseline",
            ),
        }
    }

    /// The factory names of the elements of `chain`.
    pub fn required_elements(self) -> &'static [&'static str] {
        match self {
            Self::Intel => &["videoconvert", "vapostproc", "vah264lpenc"],
            Self::Nvidia => &["cudaupload", "cudaconvert", "nvh264enc"],
            Self::Cpu => &["videoconvert", "x264enc", "capsfilter"],
        }
    }

    /// The first element of `chain` that is not in the `GStreamer` registry.
    pub fn missing_element(self) -> Option<&'static str> {
        self.required_elements()
            .iter()
            .copied()
            .find(|name| gst::ElementFactory::find(name).is_none())
    }

    /// Whether every element of `chain` is in the `GStreamer` registry.
    pub fn is_installed(self) -> bool {
        self.missing_element().is_none()
    }

    /// Runs `chain` on two test frames of the recording's output `size`.
    /// Results are cached per encoder and size, except a timeout.
    pub async fn works(self, size: (i32, i32)) -> Result<(), String> {
        static CACHE: LazyLock<Mutex<HashMap<(H264Encoder, (i32, i32)), Result<(), String>>>> =
            LazyLock::new(Mutex::default);

        let result = cached(&CACHE, (self, size), || self.probe(size)).await;
        match &result {
            Ok(()) => tracing::info!(encoder = ?self, ?size, "The H.264 encoder works"),
            Err(reason) => {
                tracing::info!(encoder = ?self, ?size, reason, "The H.264 encoder does not work");
            }
        }
        result
    }

    async fn probe(self, (width, height): (i32, i32)) -> Result<(), ProbeError> {
        if let Some(element) = self.missing_element() {
            return Err(ProbeError::Failed(format!("`{element}` is not installed")));
        }

        let description = profile::expand(
            &format!(
                "videotestsrc num-buffers=2 ! \
                 video/x-raw,format=BGRx,width={width},height={height} ! \
                 ${{H264_ENCODER}} ! fakesink"
            ),
            self,
        );
        run_probe(description, PROBE_TIMEOUT).await
    }
}

/// Encoders to try, in order, for a preference.
pub fn candidates(pref: VideoEncoder) -> Vec<H264Encoder> {
    match pref {
        VideoEncoder::Auto => AUTO_ORDER.to_vec(),
        VideoEncoder::Intel => vec![H264Encoder::Intel],
        VideoEncoder::Nvidia => vec![H264Encoder::Nvidia],
        VideoEncoder::Cpu => vec![H264Encoder::Cpu],
    }
}

/// Tries the candidates of `pref` in order and stops at the first that
/// works. On failure, the reason of the last one.
pub async fn resolve<F, Fut>(pref: VideoEncoder, works: F) -> Result<H264Encoder, String>
where
    F: Fn(H264Encoder) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let mut last_reason = String::new();
    for encoder in candidates(pref) {
        match works(encoder).await {
            Ok(()) => return Ok(encoder),
            Err(reason) => last_reason = reason,
        }
    }
    Err(last_reason)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeError {
    Failed(String),
    TimedOut,
}

impl ProbeError {
    fn reason(self) -> String {
        match self {
            Self::Failed(reason) => reason,
            Self::TimedOut => format!(
                "The encoder did not answer in {} seconds",
                PROBE_TIMEOUT.as_secs()
            ),
        }
    }
}

/// Returns the cached result for `key`, or runs `probe` and caches its result
/// unless it timed out.
async fn cached<K, F, Fut>(
    cache: &Mutex<HashMap<K, Result<(), String>>>,
    key: K,
    probe: F,
) -> Result<(), String>
where
    K: Eq + Hash,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), ProbeError>>,
{
    if let Some(result) = lock(cache).get(&key) {
        return result.clone();
    }

    match probe().await {
        Err(ProbeError::TimedOut) => Err(ProbeError::TimedOut.reason()),
        result => {
            let result = result.map_err(ProbeError::reason);
            lock(cache).insert(key, result.clone());
            result
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Plays `description` until it ends, off the main thread, for at most
/// `timeout`, and leaves it in `Null`.
async fn run_probe(description: String, timeout: Duration) -> Result<(), ProbeError> {
    // The thread also gives up after `timeout`, but starting an encoder can
    // block in `set_state` before that.
    let handle = gio::spawn_blocking(move || play_until_end(&description, timeout));
    match glib::future_with_timeout(timeout, handle).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(ProbeError::Failed("The probe panicked".to_string())),
        Err(_) => Err(ProbeError::TimedOut),
    }
}

fn play_until_end(description: &str, timeout: Duration) -> Result<(), ProbeError> {
    let pipeline = gst::parse::launch(description)
        .map_err(|err| ProbeError::Failed(err.message().to_string()))?;
    let bus = pipeline.bus().unwrap();

    let wait = if pipeline.set_state(gst::State::Playing).is_ok() {
        gst::ClockTime::try_from(timeout).unwrap_or(gst::ClockTime::MAX)
    } else {
        // The reason is on the bus.
        gst::ClockTime::ZERO
    };
    let result = match bus.timed_pop_filtered(
        wait,
        &[gst::MessageType::Eos, gst::MessageType::Error],
    ) {
        Some(message) => match message.view() {
            gst::MessageView::Error(err) => Err(ProbeError::Failed(match err.debug() {
                Some(debug) => format!("{} ({debug})", err.error()),
                None => err.error().to_string(),
            })),
            _ => Ok(()),
        },
        None if wait == gst::ClockTime::ZERO => Err(ProbeError::Failed(
            "The pipeline failed to start".to_string(),
        )),
        None => Err(ProbeError::TimedOut),
    };

    if let Err(err) = pipeline.set_state(gst::State::Null) {
        tracing::warn!("Failed to stop the encoder probe: {:?}", err);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, future::ready, time::Instant};

    use super::*;

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        glib::MainContext::new().block_on(future)
    }

    /// Resolves `pref` with a fake probe where only `working` work, and
    /// returns the result and the encoders tried.
    fn resolve_with(
        pref: VideoEncoder,
        working: &[H264Encoder],
    ) -> (Result<H264Encoder, String>, Vec<H264Encoder>) {
        let tried = RefCell::new(Vec::new());
        let result = block_on(resolve(pref, |encoder| {
            tried.borrow_mut().push(encoder);
            ready(if working.contains(&encoder) {
                Ok(())
            } else {
                Err(format!("{encoder:?} failed"))
            })
        }));
        (result, tried.into_inner())
    }

    #[test]
    fn auto_tries_the_cpu_last() {
        assert_eq!(candidates(VideoEncoder::Auto), AUTO_ORDER);
        assert_eq!(AUTO_ORDER.last(), Some(&H264Encoder::Cpu));
    }

    #[test]
    fn a_forced_encoder_is_the_only_candidate() {
        assert_eq!(candidates(VideoEncoder::Intel), [H264Encoder::Intel]);
        assert_eq!(candidates(VideoEncoder::Nvidia), [H264Encoder::Nvidia]);
        assert_eq!(candidates(VideoEncoder::Cpu), [H264Encoder::Cpu]);
    }

    #[test]
    fn auto_falls_back_to_the_cpu() {
        let (result, tried) = resolve_with(VideoEncoder::Auto, &[H264Encoder::Cpu]);
        assert_eq!(result, Ok(H264Encoder::Cpu));
        assert_eq!(tried, AUTO_ORDER);
    }

    #[test]
    fn auto_stops_at_the_first_that_works() {
        let (result, tried) = resolve_with(VideoEncoder::Auto, &AUTO_ORDER);
        assert_eq!(result, Ok(AUTO_ORDER[0]));
        assert_eq!(tried, [AUTO_ORDER[0]]);
    }

    #[test]
    fn a_forced_encoder_that_fails_is_an_error() {
        let (result, tried) = resolve_with(VideoEncoder::Intel, &[H264Encoder::Cpu]);
        assert_eq!(result, Err("Intel failed".to_string()));
        assert_eq!(tried, [H264Encoder::Intel]);
    }

    #[test]
    fn auto_with_nothing_working_gives_the_last_reason() {
        let (result, _) = resolve_with(VideoEncoder::Auto, &[]);
        assert_eq!(result, Err("Cpu failed".to_string()));
    }

    /// Runs `cached` twice on the same key with a probe that returns
    /// `result`, and returns how many times the probe ran.
    fn probe_runs(result: Result<(), ProbeError>) -> (Result<(), String>, usize) {
        let cache = Mutex::new(HashMap::new());
        let runs = RefCell::new(0);
        let mut last = Ok(());
        for _ in 0..2 {
            last = block_on(cached(&cache, "key", || {
                *runs.borrow_mut() += 1;
                ready(result.clone())
            }));
        }
        (last, runs.into_inner())
    }

    #[test]
    fn a_working_encoder_is_probed_once() {
        assert_eq!(probe_runs(Ok(())), (Ok(()), 1));
    }

    #[test]
    fn a_failing_encoder_is_probed_once() {
        let (result, runs) = probe_runs(Err(ProbeError::Failed("no device".to_string())));
        assert_eq!(result, Err("no device".to_string()));
        assert_eq!(runs, 1);
    }

    #[test]
    fn a_timeout_is_not_cached() {
        let (result, runs) = probe_runs(Err(ProbeError::TimedOut));
        assert!(result.unwrap_err().contains("did not answer"));
        assert_eq!(runs, 2);
    }

    #[test]
    fn a_probe_that_never_ends_times_out() {
        gst::init().unwrap();

        let start = Instant::now();
        let result = block_on(run_probe(
            "videotestsrc is-live=true ! fakesink".to_string(),
            Duration::from_millis(300),
        ));
        let elapsed = start.elapsed();

        assert_eq!(result, Err(ProbeError::TimedOut));
        assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
    }

    #[test]
    fn a_probe_that_cannot_negotiate_fails_with_the_reason() {
        gst::init().unwrap();

        let result = block_on(run_probe(
            "videotestsrc num-buffers=2 ! video/x-raw,format=BGRx ! audioconvert ! fakesink"
                .to_string(),
            PROBE_TIMEOUT,
        ));

        let Err(ProbeError::Failed(reason)) = result else {
            panic!("{result:?}");
        };
        assert!(!reason.is_empty());
    }

    #[test]
    fn the_cpu_encoder_works() {
        gst::init().unwrap();
        if !H264Encoder::Cpu.is_installed() {
            eprintln!("Skipped: x264enc is not installed");
            return;
        }

        assert_eq!(block_on(H264Encoder::Cpu.works((1280, 720))), Ok(()));
    }

    #[test]
    fn required_elements_are_the_chain_elements() {
        for encoder in [H264Encoder::Intel, H264Encoder::Nvidia, H264Encoder::Cpu] {
            let elements = encoder
                .chain()
                .split('!')
                .filter_map(|part| part.split_whitespace().next())
                .filter(|part| !part.starts_with("video/"))
                .collect::<Vec<_>>();
            assert_eq!(elements, encoder.required_elements(), "{encoder:?}");
        }
    }
}
