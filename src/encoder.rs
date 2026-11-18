//! The H.264 encoders a recording can use, and the probe that tells whether
//! one works before recording with it (ADR 0006).

use std::{
    collections::HashMap,
    future::Future,
    hash::Hash,
    panic::{self, AssertUnwindSafe},
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use futures_channel::oneshot;
use gst::prelude::*;
use gtk::glib;

use crate::{profile, settings::VideoEncoder};

/// How long a probe may take. A sleeping NVIDIA card took several seconds to
/// start in docs/perf/2026-11-18-capture-bench.md.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The encoders Automatic tries, in order, with the CPU always last.
///
/// Intel first, as the spec asks when it keeps 60 FPS: it uses less battery
/// and leaves the NVIDIA card asleep. In the synthetic-source run of
/// docs/perf/2026-11-18-capture-bench.md, Intel with the CPU conversion to
/// NV12 kept 60 FPS with no frame lost. Provisional until the run with the
/// KDE portal confirms it.
pub const AUTO_ORDER: [H264Encoder; 3] =
    [H264Encoder::Intel, H264Encoder::Nvidia, H264Encoder::Cpu];

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
    ///
    /// The hardware encoders use a constant quantizer of 17, the `qp-max` of
    /// the CPU one, so that text on screen looks the same with any of them.
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
                "vah264lpenc rate-control=cqp qpi=17 qpp=17 qpb=17",
            ),
            Self::Nvidia => concat!(
                "cudaupload ! ",
                "cudaconvert ! ",
                "video/x-raw(memory:CUDAMemory),format=NV12 ! ",
                "nvh264enc rc-mode=constqp qp-const=17",
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

    /// The factory name of the element of `chain` that encodes.
    pub fn encoder_element(self) -> &'static str {
        match self {
            Self::Intel => "vah264lpenc",
            Self::Nvidia => "nvh264enc",
            Self::Cpu => "x264enc",
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

    /// Runs `chain` on two test frames of `size`. Results are cached per
    /// encoder and size, except a timeout, and an encoder never runs two
    /// probes at once.
    pub async fn works(self, size: (i32, i32)) -> Result<(), String> {
        static CACHE: LazyLock<ProbeCache<H264Encoder, (i32, i32)>> = LazyLock::new(Mutex::default);

        let result = cached(&CACHE, self, size, PROBE_TIMEOUT, move || self.probe(size)).await;
        match &result {
            Ok(()) => tracing::info!(encoder = ?self, ?size, "The H.264 encoder works"),
            Err(reason) => {
                tracing::info!(encoder = ?self, ?size, reason, "The H.264 encoder does not work");
            }
        }
        result
    }

    /// Blocks until the probe ends, for at most `PROBE_TIMEOUT`.
    fn probe(self, (width, height): (i32, i32)) -> Result<(), ProbeError> {
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
        play_until_end(&description, PROBE_TIMEOUT)
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

/// The probes of encoders `E` at sizes `S`.
type ProbeCache<E, S> = Mutex<Probes<E, S>>;

#[derive(Debug)]
struct Probes<E, S> {
    /// The results of the probes that did not time out, by encoder and size.
    results: HashMap<(E, S), Result<(), String>>,
    /// The probe whose thread still runs, at most one per encoder whatever
    /// the size: a stuck driver must not take a thread per size (codex code
    /// r2 #2).
    in_flight: HashMap<E, InFlight<S>>,
    next_id: u64,
}

impl<E, S> Default for Probes<E, S> {
    fn default() -> Self {
        Self {
            results: HashMap::new(),
            in_flight: HashMap::new(),
            next_id: 0,
        }
    }
}

#[derive(Debug)]
struct InFlight<S> {
    id: u64,
    /// When its thread was launched.
    started: Instant,
    /// The requests waiting for it. Each gets the size probed with the
    /// result, as one of another size probes its own after it.
    waiting: Vec<oneshot::Sender<(S, Result<(), String>)>>,
    /// A request timed out after waiting for it its whole timeout: the
    /// thread may be stuck in the driver, so the next requests time out at
    /// once (fable code r2 #5). A request that launched it late in its own
    /// timeout says nothing about it (fable code r3 #3).
    stuck: bool,
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

/// Returns the cached result for `encoder` at `size`, or waits at most
/// `timeout` for `probe`, which runs in a thread of its own.
///
/// The thread outlives a timeout: an encoder can block in `set_state`. While
/// it runs, a new request for the same encoder launches no other thread: of
/// the same size, it gets the result of the running one; of another size,
/// it launches its own once the running one ends. After a request timed out
/// with it running for `timeout` at least, the next ones time out at once. When it ends, its result
/// is cached, unless it timed out itself.
async fn cached<E, S, F>(
    cache: &'static ProbeCache<E, S>,
    encoder: E,
    size: S,
    timeout: Duration,
    probe: F,
) -> Result<(), String>
where
    E: Eq + Hash + Clone + Send + 'static,
    S: Eq + Hash + Clone + Send + 'static,
    F: FnOnce() -> Result<(), ProbeError> + Send + 'static,
{
    let deadline = Instant::now() + timeout;
    // The whole `timeout` the first time: the timer counts whole
    // milliseconds, and what is left of it would end a bit early.
    let mut left = timeout;
    let mut probe = Some(probe);
    loop {
        let (sender, receiver) = oneshot::channel();
        let id = {
            let mut probes = lock(cache);
            if let Some(result) = probes.results.get(&(encoder.clone(), size.clone())) {
                return result.clone();
            }
            let next_id = probes.next_id;
            if let Some(in_flight) = probes.in_flight.get_mut(&encoder) {
                if in_flight.stuck {
                    return Err(ProbeError::TimedOut.reason());
                }
                in_flight.waiting.push(sender);
                in_flight.id
            } else {
                // Only a probe of another size ended before this one: this
                // request has not launched its own yet.
                let Some(probe) = probe.take() else {
                    return Err("The probe ended with no result".to_string());
                };
                probes.next_id += 1;
                probes.in_flight.insert(
                    encoder.clone(),
                    InFlight {
                        id: next_id,
                        started: Instant::now(),
                        waiting: vec![sender],
                        stuck: false,
                    },
                );
                let thread_encoder = encoder.clone();
                let thread_size = size.clone();
                let spawned = std::thread::Builder::new()
                    .name("kooha-encoder-probe".to_string())
                    .spawn(move || {
                        let result =
                            panic::catch_unwind(AssertUnwindSafe(probe)).unwrap_or_else(|_| {
                                Err(ProbeError::Failed("The probe panicked".to_string()))
                            });
                        finish(cache, thread_encoder, next_id, &thread_size, result);
                    });
                if let Err(err) = spawned {
                    // No thread finishes this entry.
                    probes.in_flight.remove(&encoder);
                    return Err(format!("Failed to start the encoder probe: {err}"));
                }
                next_id
            }
        };

        match glib::future_with_timeout(left, receiver).await {
            Ok(Ok((probed, result))) if probed == size => return result,
            // The probe of another size ended: now this one's, in what is
            // left of `timeout`.
            Ok(Ok(_)) => left = deadline.saturating_duration_since(Instant::now()),
            Ok(Err(oneshot::Canceled)) => return Err("The probe ended with no result".to_string()),
            Err(_) => {
                let mut probes = lock(cache);
                if let Some(in_flight) = probes.in_flight.get_mut(&encoder)
                    && in_flight.id == id
                {
                    if in_flight.started.elapsed() >= timeout {
                        in_flight.stuck = true;
                    }
                    // Those that gave up, this one among them.
                    in_flight.waiting.retain(|sender| !sender.is_canceled());
                }
                return Err(ProbeError::TimedOut.reason());
            }
        }
    }
}

/// Caches the result of the probe `id` of `encoder` at `size`, whose thread
/// ends, and hands it to the requests still waiting.
fn finish<E: Eq + Hash, S: Eq + Hash + Clone>(
    cache: &ProbeCache<E, S>,
    encoder: E,
    id: u64,
    size: &S,
    result: Result<(), ProbeError>,
) {
    let timed_out = result == Err(ProbeError::TimedOut);
    let result = result.map_err(ProbeError::reason);

    let mut probes = lock(cache);
    let waiting = match probes.in_flight.remove(&encoder) {
        Some(in_flight) => {
            debug_assert_eq!(in_flight.id, id);
            in_flight.waiting
        }
        None => {
            debug_assert!(false, "the probe {id} was not in flight");
            Vec::new()
        }
    };
    if !timed_out {
        probes
            .results
            .insert((encoder, size.clone()), result.clone());
    }
    drop(probes);

    for sender in waiting {
        // The request may have timed out already.
        let _ = sender.send((size.clone(), result.clone()));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Plays `description` until it ends, for at most `timeout` once it plays,
/// and leaves it in `Null`.
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
    let result =
        match bus.timed_pop_filtered(wait, &[gst::MessageType::Eos, gst::MessageType::Error]) {
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
    use std::{
        cell::RefCell,
        future::ready,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::Instant,
    };

    use super::*;

    /// The quantizer of `chain` for every encoder.
    const CONSTANT_QP: u32 = 17;

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
    fn auto_tries_intel_then_nvidia_then_the_cpu() {
        assert_eq!(
            AUTO_ORDER,
            [H264Encoder::Intel, H264Encoder::Nvidia, H264Encoder::Cpu]
        );
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

    /// A cache that lives as long as the probe threads that write to it.
    fn new_cache() -> &'static ProbeCache<&'static str, u32> {
        Box::leak(Box::default())
    }

    /// Runs `cached` twice on the same key with a probe that returns
    /// `result`, and returns how many times the probe ran.
    fn probe_runs(result: &Result<(), ProbeError>) -> (Result<(), String>, usize) {
        let cache = new_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let mut last = Ok(());
        for _ in 0..2 {
            let runs = Arc::clone(&runs);
            let result = result.clone();
            last = block_on(cached(cache, "key", 0, PROBE_TIMEOUT, move || {
                runs.fetch_add(1, Ordering::SeqCst);
                result
            }));
        }
        (last, runs.load(Ordering::SeqCst))
    }

    #[test]
    fn a_working_encoder_is_probed_once() {
        assert_eq!(probe_runs(&Ok(())), (Ok(()), 1));
    }

    #[test]
    fn a_failing_encoder_is_probed_once() {
        let (result, runs) = probe_runs(&Err(ProbeError::Failed("no device".to_string())));
        assert_eq!(result, Err("no device".to_string()));
        assert_eq!(runs, 1);
    }

    #[test]
    fn a_timeout_is_not_cached() {
        let (result, runs) = probe_runs(&Err(ProbeError::TimedOut));
        assert!(result.unwrap_err().contains("did not answer"));
        assert_eq!(runs, 2);
    }

    #[test]
    fn a_probe_that_panics_fails_and_is_cached() {
        let (result, runs) = probe_runs_panicking();
        assert_eq!(result, Err("The probe panicked".to_string()));
        assert_eq!(runs, 1);
    }

    fn probe_runs_panicking() -> (Result<(), String>, usize) {
        let cache = new_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let mut last = Ok(());
        for _ in 0..2 {
            let runs = Arc::clone(&runs);
            last = block_on(cached(cache, "key", 0, PROBE_TIMEOUT, move || {
                runs.fetch_add(1, Ordering::SeqCst);
                panic!("the driver crashed");
            }));
        }
        (last, runs.load(Ordering::SeqCst))
    }

    /// A probe that blocks until `release` is sent `result`, counting its
    /// runs in `runs`.
    fn stuck_probe(
        runs: &Arc<AtomicUsize>,
        release: &Arc<Mutex<mpsc::Receiver<Result<(), ProbeError>>>>,
    ) -> impl FnOnce() -> Result<(), ProbeError> + Send + 'static {
        let runs = Arc::clone(runs);
        let release = Arc::clone(release);
        move || {
            runs.fetch_add(1, Ordering::SeqCst);
            lock(&release).recv().unwrap()
        }
    }

    /// Runs `request` and returns its result and how long it took.
    fn timed<T>(request: impl Future<Output = T>) -> (T, Duration) {
        let start = Instant::now();
        let result = block_on(request);
        (result, start.elapsed())
    }

    #[test]
    fn a_stuck_probe_answers_the_next_requests_at_once() {
        const TIMEOUT: Duration = Duration::from_millis(300);
        let cache = new_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));

        let (result, elapsed) = timed(cached(
            cache,
            "key",
            0,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        assert!(result.unwrap_err().contains("did not answer"));
        assert!(elapsed >= TIMEOUT, "{elapsed:?}");
        assert!(elapsed < TIMEOUT * 5, "{elapsed:?}");

        // Once a request timed out, the next ones, of any size, don't wait
        // for the stuck thread nor launch another (fable code r2 #5, codex
        // code r2 #2).
        for size in [0, 1, 0] {
            let (result, elapsed) = timed(cached(
                cache,
                "key",
                size,
                TIMEOUT,
                stuck_probe(&runs, &released),
            ));
            assert!(result.unwrap_err().contains("did not answer"));
            assert!(elapsed < Duration::from_millis(100), "{elapsed:?}");
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // The requests that gave up wait no more.
        assert!(lock(cache).in_flight["key"].waiting.is_empty());

        // Once the thread ends, its result is the cached one.
        release.send(Ok(())).unwrap();
        let start = Instant::now();
        while !lock(cache).results.contains_key(&("key", 0)) {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(10));
        }
        let (result, elapsed) = timed(cached(
            cache,
            "key",
            0,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        assert_eq!(result, Ok(()));
        assert!(elapsed < TIMEOUT, "{elapsed:?}");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // And another size is probed again.
        release.send(Ok(())).unwrap();
        let (result, _) = timed(cached(
            cache,
            "key",
            1,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        assert_eq!(result, Ok(()));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    /// Waits until `runs` is `n`.
    fn wait_for_runs(runs: &AtomicUsize, n: usize) {
        let start = Instant::now();
        while runs.load(Ordering::SeqCst) < n {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_second_request_gets_the_result_of_the_probe_in_flight() {
        const TIMEOUT: Duration = Duration::from_millis(1000);
        let cache = new_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));

        let first = std::thread::spawn({
            let probe = stuck_probe(&runs, &released);
            move || timed(cached(cache, "key", 0, TIMEOUT, probe)).0
        });
        wait_for_runs(&runs, 1);

        // The probe ends 100 ms into the second request.
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            release
                .send(Err(ProbeError::Failed("no device".to_string())))
                .unwrap();
        });
        let (result, elapsed) = timed(cached(
            cache,
            "key",
            0,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        sender.join().unwrap();

        assert_eq!(result, Err("no device".to_string()));
        assert_eq!(first.join().unwrap(), Err("no device".to_string()));
        assert!(elapsed >= Duration::from_millis(100), "{elapsed:?}");
        assert!(elapsed < TIMEOUT, "{elapsed:?}");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn another_size_waits_for_the_probe_in_flight_before_its_own() {
        const TIMEOUT: Duration = Duration::from_millis(1000);
        let cache = new_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));

        let first = std::thread::spawn({
            let probe = stuck_probe(&runs, &released);
            move || timed(cached(cache, "key", 0, TIMEOUT, probe)).0
        });
        wait_for_runs(&runs, 1);
        let second = std::thread::spawn({
            let probe = stuck_probe(&runs, &released);
            move || timed(cached(cache, "key", 1, TIMEOUT, probe))
        });

        // No second thread of the same encoder while the first one runs
        // (codex code r2 #2).
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        release.send(Ok(())).unwrap();
        wait_for_runs(&runs, 2);
        release
            .send(Err(ProbeError::Failed("too big".to_string())))
            .unwrap();

        assert_eq!(first.join().unwrap(), Ok(()));
        let (result, elapsed) = second.join().unwrap();
        assert_eq!(result, Err("too big".to_string()));
        assert!(elapsed >= Duration::from_millis(100), "{elapsed:?}");
        assert!(elapsed < TIMEOUT, "{elapsed:?}");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        let probes = lock(cache);
        assert_eq!(probes.results[&("key", 0)], Ok(()));
        assert_eq!(probes.results[&("key", 1)], Err("too big".to_string()));
    }

    #[test]
    fn a_probe_launched_late_in_a_request_is_not_stuck() {
        const TIMEOUT: Duration = Duration::from_millis(400);
        let cache = new_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));

        let first = std::thread::spawn({
            let probe = stuck_probe(&runs, &released);
            move || block_on(cached(cache, "key", 0, Duration::from_secs(5), probe))
        });
        wait_for_runs(&runs, 1);

        // Another size waits for the first probe almost all its timeout,
        // launches its own and times out 50 ms later (fable code r3 #3).
        let start = Instant::now();
        let sender = std::thread::spawn({
            let release = release.clone();
            move || {
                std::thread::sleep(TIMEOUT - Duration::from_millis(50));
                release.send(Ok(())).unwrap();
            }
        });
        let (result, elapsed) = timed(cached(
            cache,
            "key",
            1,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        sender.join().unwrap();
        assert_eq!(first.join().unwrap(), Ok(()));
        assert!(result.unwrap_err().contains("did not answer"));
        // What is left of the timeout counts whole milliseconds.
        assert!(elapsed >= TIMEOUT - Duration::from_millis(2), "{elapsed:?}");
        assert!(start.elapsed() < TIMEOUT * 2, "{:?}", start.elapsed());
        assert_eq!(runs.load(Ordering::SeqCst), 2);

        // Its probe ran 50 ms only: the next request waits for it.
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            release.send(Ok(())).unwrap();
        });
        let (result, elapsed) = timed(cached(
            cache,
            "key",
            1,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        sender.join().unwrap();
        assert_eq!(result, Ok(()));
        assert!(elapsed >= Duration::from_millis(100), "{elapsed:?}");
        assert!(elapsed < TIMEOUT, "{elapsed:?}");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_stuck_probe_that_ends_timed_out_is_launched_again() {
        const TIMEOUT: Duration = Duration::from_millis(200);
        let cache = new_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));

        let (result, _) = timed(cached(
            cache,
            "key",
            0,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        assert!(result.unwrap_err().contains("did not answer"));
        release.send(Err(ProbeError::TimedOut)).unwrap();
        let start = Instant::now();
        while lock(cache).in_flight.contains_key("key") {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(10));
        }

        release.send(Ok(())).unwrap();
        let (result, _) = timed(cached(
            cache,
            "key",
            0,
            TIMEOUT,
            stuck_probe(&runs, &released),
        ));
        assert_eq!(result, Ok(()));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_probe_that_never_ends_times_out() {
        const TIMEOUT: Duration = Duration::from_millis(300);
        gst::init().unwrap();

        let description = "videotestsrc is-live=true ! fakesink";
        let (result, elapsed) = timed(cached(new_cache(), "key", 0, TIMEOUT, move || {
            play_until_end(description, TIMEOUT)
        }));

        assert!(result.unwrap_err().contains("did not answer"));
        assert!(elapsed >= TIMEOUT, "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
    }

    #[test]
    fn a_probe_that_cannot_negotiate_fails_with_the_reason() {
        gst::init().unwrap();

        let result = play_until_end(
            "videotestsrc num-buffers=2 ! video/x-raw,format=BGRx ! audioconvert ! fakesink",
            PROBE_TIMEOUT,
        );

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

    /// Parses `encoder`'s chain as the recording does, failing on any
    /// unknown property, and returns its encoder element, or `None` when the
    /// chain is not installed.
    fn parsed_encoder(encoder: H264Encoder, factory: &str) -> Option<gst::Element> {
        gst::init().unwrap();
        if !encoder.is_installed() {
            eprintln!("Skipped: {encoder:?} is not installed");
            return None;
        }

        let bin = gst::parse::bin_from_description_with_name_full(
            &profile::expand("${H264_ENCODER}", encoder),
            false,
            "",
            None,
            gst::ParseFlags::FATAL_ERRORS,
        )
        .unwrap()
        .downcast::<gst::Bin>()
        .unwrap();
        bin.iterate_elements()
            .into_iter()
            .flatten()
            .find(|element| {
                element
                    .factory()
                    .is_some_and(|element_factory| element_factory.name() == factory)
            })
    }

    fn enum_nick(element: &gst::Element, property: &str) -> String {
        let value = element.property_value(property);
        glib::EnumValue::from_value(&value)
            .map(|(_, value)| value.nick().to_string())
            .unwrap()
    }

    #[test]
    fn intel_encodes_with_the_constant_qp_of_the_cpu() {
        let Some(element) = parsed_encoder(H264Encoder::Intel, "vah264lpenc") else {
            return;
        };

        assert_eq!(enum_nick(&element, "rate-control"), "cqp");
        for property in ["qpi", "qpp", "qpb"] {
            assert_eq!(element.property::<u32>(property), CONSTANT_QP, "{property}");
        }
    }

    #[test]
    fn nvidia_encodes_with_the_constant_qp_of_the_cpu() {
        let Some(element) = parsed_encoder(H264Encoder::Nvidia, "nvh264enc") else {
            return;
        };

        assert_eq!(enum_nick(&element, "rc-mode"), "constqp");
        assert_eq!(element.property::<i32>("qp-const"), CONSTANT_QP as i32);
    }

    #[test]
    fn the_cpu_caps_its_qp_at_the_constant_qp() {
        let Some(element) = parsed_encoder(H264Encoder::Cpu, "x264enc") else {
            return;
        };

        assert_eq!(element.property::<u32>("qp-max"), CONSTANT_QP);
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
            assert!(elements.contains(&encoder.encoder_element()), "{encoder:?}");
        }
    }
}
