#!/usr/bin/python3
"""Measure variants of Kooha's capture pipeline.

Records the same 1280x720 area at (200, 160) with each pipeline variant and
prints one Markdown table per frame rate: frames the source delivered, frames
lost on the way to the file, real FPS at the encoder output, CPU of this
process, and whether the variant started at all.

Usage:

    /usr/bin/python3 -I tools/bench-capture.py            # test source, all variants
    /usr/bin/python3 -I tools/bench-capture.py --variant today --seconds 2 --fps 30
    /usr/bin/python3 -I tools/bench-capture.py --fps 60 --fps 30 --out /tmp/bench.md
    /usr/bin/python3 -I tools/bench-capture.py --source portal --fps 60 --fps 30
    /usr/bin/python3 -I tools/bench-capture.py --probe-session  # portal check only

It needs the system Python, which has PyGObject (`gi`) and GStreamer 1.0; a
`python3` from another prefix (Homebrew, a venv) usually lacks them.

Sources:

    --source test    videotestsrc pattern=ball at 2560x1600 BGRx. It touches
                     neither the portal nor the graphical session.
    --source portal  A ScreenCast portal session for one monitor, restored with
                     Kooha's `screencast-restore-token`, so KDE shows no dialog
                     once permission was granted. Run it from the user's
                     graphical session. Once the stream arrives, it opens a
                     fullscreen window with a ball that bounces inside the
                     measured area, on the monitor whose logical (within 1
                     px) or physical (within 2 px) size matches the stream's
                     reported size (`--no-ball-window` leaves it out). All the
                     variants and frame rates share one session. A pass where
                     the source delivered under 90 % of the requested frames
                     in the `today` variant is marked as discarded: the
                     screen did not change often enough to measure.

The restore token (portal and --probe-session). Before opening the session the
current token is saved to /tmp/kooha-bench-token.bak. Start spends the token:
KDE hands out a successor with every session. Whenever Start succeeds and the
stream is a monitor, the script writes the successor to Kooha's key as soon as
Start answers, before any pipeline runs, however long Start took: the original
token is spent either way, and a restore without a dialog may be slow. It says
when it leaves the key alone (the stream is not a monitor, or KDE gave no
successor). The seconds Start took are printed, as a fact only.

The .bak is only restored by hand
(`gsettings set io.github.seadve.Kooha screencast-restore-token "$(cat
/tmp/kooha-bench-token.bak)"`). That helps when KDE asks every time because
it sees another app id than Kooha's: then Kooha's original token is intact.

    --probe-session    Only opens and closes the session, to check whether KDE
                       accepts Kooha's token from a process that is not Kooha.
    --allow-dialog     Accepted and ignored: the script no longer guesses
    --write-token      whether KDE showed a dialog, and always writes the
                       successor of a monitor stream. Kept so that older
                       command lines still run.
    --with-kde-region  At the end, opens a second session without the token for
                       KDE's own region sharing (pick a 1280x720 region in the
                       dialog) and records it as the `kde-region` variant. Its
                       token is never written. Unverified: which source types
                       make KDE offer a region; the script asks for monitor and
                       virtual.

Variants (`--variant`, repeatable, `all` by default):

    today              videoflip -> videorate -> videoscale -> videocrop ->
                       x264 (profiles.yml), as Kooha did before the change
    crop-first         videorate -> videocrop -> x264
    crop-first-queues  Kooha's pipeline after the change: a leaky 3-buffer
                       queue between videoflip and videorate (videorate fills
                       what it drops) and a 30-buffer queue before the encoder
    intel              crop-first-queues with vapostproc + vah264lpenc
    intel-convert      intel with videoconvert to NV12 in the CPU first, so
                       vapostproc gets a small system-memory frame
    nvidia             crop-first-queues with cudaupload + cudaconvert + nvh264enc
    intel-dmabuf       (portal only) pipewiresrc in DMABuf memory, cropped by
                       vapostproc through GstVideoCropMeta, + vah264lpenc
    nvidia-dmabuf      (portal only) pipewiresrc in DMABuf memory, imported by
                       glupload, converted by glcolorconvert, + nvh264enc

A variant whose caps do not negotiate is reported with the error. Each
variant writes /tmp/kooha-bench-<variant>-<fps>.mp4.

Columns:

    startup s      from PLAYING to the first encoded buffer (a GPU may need
                   seconds to wake up); the measured window starts there
    source fps     buffers out of the source per second, in the window
    fps            buffers out of the encoder per second, in the window
    source frames  buffers out of the source, up to EOS
    frames         buffers out of the encoder, up to EOS
    lost           frames the source delivered that did not reach the file,
                   leaving out videorate's own work: frames lost before
                   videorate (source frames - videorate in) plus frames lost
                   after it (videorate out - frames)
    dup, drop      frames videorate duplicated or dropped to keep the rate;
                   dup also fills what the leaky queue threw away
    cpu %          utime + stime of this process (/proc/self/stat) in the
                   window, in % of one core; the test source's own rendering
                   counts too
"""

import argparse
import os
import subprocess
import sys
import time

try:
    import gi

    gi.require_version("Gst", "1.0")
    from gi.repository import Gio, GLib, Gst
except (ImportError, ValueError) as e:
    sys.exit(
        f"{sys.executable} lacks PyGObject or GStreamer 1.0 ({e}). "
        "Run the script with /usr/bin/python3."
    )

AREA = (200, 160, 1280, 720)  # x, y, width, height in stream pixels
TEST_SIZE = (2560, 1600)
ALL_VARIANTS = [
    "today",
    "crop-first",
    "crop-first-queues",
    "intel",
    "intel-convert",
    "nvidia",
    "intel-dmabuf",
    "nvidia-dmabuf",
]
PORTAL_ONLY = {"intel-dmabuf", "nvidia-dmabuf"}
KDE_REGION = "kde-region"
STARTUP_TIMEOUT = 15
MIN_SOURCE_SHARE = 0.9
TOKEN_BACKUP = "/tmp/kooha-bench-token.bak"
KOOHA_SCHEMA = "io.github.seadve.Kooha"
RESTORE_TOKEN_KEY = "screencast-restore-token"
N_THREADS = max(1, min(os.cpu_count() or 1, 64))

# The same queues as src/pipeline.rs.
CAPTURE_QUEUE = (
    "queue name=capture-queue max-size-buffers=3 max-size-time=0 "
    "max-size-bytes=0 leaky=downstream"
)
ENCODER_QUEUE = (
    "queue name=videoenc-queue max-size-buffers=30 max-size-time=0 max-size-bytes=0"
)
X264 = (
    f"videoconvert chroma-mode=none dither=none matrix-mode=output-only "
    f"n-threads={N_THREADS} ! "
    f"x264enc qp-max=17 speed-preset=ultrafast threads={N_THREADS} ! "
    "capsfilter caps=video/x-h264,profile=baseline ! queue ! h264parse name=encoded"
)
INTEL = (
    "vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! "
    "vah264lpenc ! h264parse name=encoded"
)
NVIDIA = (
    "cudaupload ! cudaconvert ! video/x-raw(memory:CUDAMemory),format=NV12 ! "
    "nvh264enc ! h264parse name=encoded"
)
NVIDIA_GL = (
    "glupload ! glcolorconvert ! video/x-raw(memory:GLMemory),format=NV12 ! "
    "nvh264enc ! h264parse name=encoded"
)
MUXER = "mp4mux fragment-duration=500 fragment-mode=first-moov-then-finalise"


def even(n):
    return n // 2 * 2


def crop_sides(frame):
    """videocrop's right and bottom that cut AREA out of a frame of this size."""
    x, y, width, height = AREA
    frame_w, frame_h = frame
    return max(0, frame_w - x - width), max(0, frame_h - y - height)


def caps_frame(caps):
    s = caps.get_structure(0)
    found_w, width = s.get_int("width")
    found_h, height = s.get_int("height")
    return (width, height) if found_w and found_h else None


def on_caps(element, apply):
    """Calls `apply(frame)` on the caps that reach `element`'s sink pad, before
    the element configures itself with them.
    """

    def probe(_pad, info):
        event = info.get_event()
        if event is not None and event.type == Gst.EventType.CAPS:
            frame = caps_frame(event.parse_caps())
            if frame is not None:
                apply(frame)
        return Gst.PadProbeReturn.OK

    element.get_static_pad("sink").add_probe(
        Gst.PadProbeType.EVENT_DOWNSTREAM, probe
    )


def source_head(source, fps, stream, dmabuf=False, test_size=TEST_SIZE):
    if source == "test":
        width, height = test_size
        return (
            "videotestsrc name=source pattern=ball is-live=true ! "
            f"video/x-raw,format=BGRx,width={width},height={height},"
            f"framerate={fps}/1"
        )
    head = (
        f"pipewiresrc name=source fd={stream['fd']} path={stream['node']} "
        "do-timestamp=true provide-clock=false keepalive-time=1000 resend-last=true"
    )
    return f"{head} ! video/x-raw(memory:DMABuf)" if dmabuf else head


def pipeline_desc(variant, source, fps, stream, test_size=TEST_SIZE, out=None):
    # right and bottom come from the negotiated frames (build_pipeline): the
    # portal's `size` may be the logical one, smaller than the frames.
    crop = f"videocrop name=crop left={AREA[0]} top={AREA[1]}"
    rate = f"videorate name=rate skip-to-first=true ! video/x-raw,framerate={fps}/1"
    head = source_head(source, fps, stream, test_size=test_size)
    flip = "videoflip video-direction=auto"
    plain_src = f"{head} ! {flip} ! {rate}"
    queued_src = f"{head} ! {flip} ! {CAPTURE_QUEUE} ! {rate}"

    if variant == "today":
        # videoscale to the even size of the frames, set by build_pipeline.
        chain = (
            f"{plain_src} ! videoscale name=scale ! capsfilter name=scale-caps ! "
            f"{crop} ! queue name=videoenc-queue ! {X264}"
        )
    elif variant == "crop-first":
        chain = f"{plain_src} ! {crop} ! queue name=videoenc-queue ! {X264}"
    elif variant == "crop-first-queues":
        chain = f"{queued_src} ! {crop} ! {ENCODER_QUEUE} ! {X264}"
    elif variant == "intel":
        chain = f"{queued_src} ! {crop} ! {ENCODER_QUEUE} ! {INTEL}"
    elif variant == "intel-convert":
        chain = (
            f"{queued_src} ! {crop} ! {ENCODER_QUEUE} ! "
            f"videoconvert n-threads={N_THREADS} ! video/x-raw,format=NV12 ! {INTEL}"
        )
    elif variant == "nvidia":
        chain = f"{queued_src} ! {crop} ! {ENCODER_QUEUE} ! {NVIDIA}"
    elif variant in ("intel-dmabuf", "nvidia-dmabuf"):
        # videoflip only takes system memory, so it is left out. videocrop
        # takes any memory and, when downstream supports it, only attaches a
        # GstVideoCropMeta for the GPU element to apply.
        dmabuf_rate = (
            "videorate name=rate skip-to-first=true ! "
            f"video/x-raw(memory:DMABuf),framerate={fps}/1"
        )
        encoder = INTEL if variant == "intel-dmabuf" else NVIDIA_GL
        chain = (
            f"{source_head(source, fps, stream, dmabuf=True)} ! {CAPTURE_QUEUE} ! "
            f"{dmabuf_rate} ! {crop} ! {ENCODER_QUEUE} ! {encoder}"
        )
    elif variant == KDE_REGION:
        # The stream already is the region: nothing to crop.
        chain = f"{queued_src} ! {ENCODER_QUEUE} ! {X264}"
    else:
        raise ValueError(variant)

    out = out or f"/tmp/kooha-bench-{variant}-{fps}.mp4"
    return f"{chain} ! {MUXER} ! filesink location={out}"


def build_pipeline(variant, source, fps, stream, test_size=TEST_SIZE, out=None):
    """Builds the variant's pipeline; raises GLib.Error if it does not parse.

    The crop, and in `today` the scale to an even size, follow the caps the
    frames arrive with, as make_area_crop does in src/pipeline.rs.
    """
    pipeline = Gst.parse_launch(
        pipeline_desc(variant, source, fps, stream, test_size, out)
    )

    crop = pipeline.get_by_name("crop")
    if crop is not None:

        def set_crop(frame):
            right, bottom = crop_sides(frame)
            crop.set_property("right", right)
            crop.set_property("bottom", bottom)

        on_caps(crop, set_crop)

    scale = pipeline.get_by_name("scale")
    if scale is not None:
        scale_caps = pipeline.get_by_name("scale-caps")

        def set_scale(frame):
            caps = f"video/x-raw,width={even(frame[0])},height={even(frame[1])}"
            scale_caps.set_property("caps", Gst.Caps.from_string(caps))

        on_caps(scale, set_scale)

    return pipeline


def process_cpu_seconds():
    with open("/proc/self/stat") as f:
        # The command name may contain spaces; the fields after it do not.
        fields = f.read().rsplit(")", 1)[1].split()
    # utime and stime are fields 14 and 15; fields[0] is field 3.
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


class Result:
    def __init__(self, variant):
        self.variant = variant
        self.started = False
        self.error = ""
        self.startup = 0.0
        self.source_fps = 0.0
        self.fps = 0.0
        self.source_frames = 0
        self.frames = 0
        self.dup = 0
        self.drop = 0
        self.rate_in = 0
        self.rate_out = 0
        self.cpu = 0.0

    @property
    def lost(self):
        # videorate pushes one extra copy of the last frame at EOS, without
        # counting it as a duplicate; its in and out counters include it.
        return (self.source_frames - self.rate_in) + (self.rate_out - self.frames)

    def row(self):
        started = "yes" if self.started else "no"
        error = self.error.replace("|", "/").replace("\n", " ")
        return (
            f"| {self.variant} | {started} | {self.startup:.1f} | "
            f"{self.source_fps:.1f} | {self.fps:.1f} | {self.source_frames} | "
            f"{self.frames} | {self.lost} | {self.dup} | {self.drop} | "
            f"{self.cpu:.0f} | {error} |"
        )


TABLE_HEADER = (
    "| variant | started | startup s | source fps | fps | source frames | frames "
    "| lost | dup | drop | cpu % | error |\n"
    "|---|---|---|---|---|---|---|---|---|---|---|---|"
)


class Counter:
    """Counts the buffers through a pad and keeps their arrival times."""

    def __init__(self):
        self.times = []
        self.cpu_at_first = None

    def probe(self, _pad, info):
        if info.get_buffer() is not None:
            if self.cpu_at_first is None:
                self.cpu_at_first = process_cpu_seconds()
            self.times.append(time.monotonic())
        return Gst.PadProbeReturn.OK

    def in_window(self, start, end):
        return [t for t in self.times if start <= t <= end]


def bus_error(msg):
    if msg.type == Gst.MessageType.ERROR:
        err, debug = msg.parse_error()
        detail = debug.splitlines()[0] if debug else ""
        return f"{err.message} ({detail})"
    return "unexpected EOS"


def run_variant(variant, source, fps, seconds, stream):
    result = Result(variant)
    try:
        pipeline = build_pipeline(variant, source, fps, stream)
    except GLib.Error as e:
        result.error = f"does not build: {e.message}"
        return result

    source_count = Counter()
    encoded_count = Counter()
    pipeline.get_by_name("source").get_static_pad("src").add_probe(
        Gst.PadProbeType.BUFFER, source_count.probe
    )
    pipeline.get_by_name("encoded").get_static_pad("src").add_probe(
        Gst.PadProbeType.BUFFER, encoded_count.probe
    )

    bus = pipeline.get_bus()
    stop_types = Gst.MessageType.ERROR | Gst.MessageType.EOS

    def poll_until(done, deadline):
        """Pops bus messages until `done()` or the deadline; returns an error."""
        while not done() and time.monotonic() < deadline:
            step = min(0.05, max(0.0, deadline - time.monotonic()))
            msg = bus.timed_pop_filtered(int(step * Gst.SECOND), stop_types)
            if msg is not None:
                return bus_error(msg)
        return None

    wall_start = time.monotonic()
    if pipeline.set_state(Gst.State.PLAYING) == Gst.StateChangeReturn.FAILURE:
        msg = bus.timed_pop_filtered(Gst.SECOND, Gst.MessageType.ERROR)
        result.error = bus_error(msg) if msg else "PLAYING failed"
        pipeline.set_state(Gst.State.NULL)
        return result

    error = poll_until(lambda: bool(encoded_count.times), wall_start + STARTUP_TIMEOUT)
    if error is None and not encoded_count.times:
        error = f"no encoded buffer within {STARTUP_TIMEOUT} s"

    if encoded_count.times:
        first = encoded_count.times[0]
        result.startup = first - wall_start
        error = error or poll_until(lambda: False, first + seconds)
        window_end = time.monotonic()
        cpu_used = process_cpu_seconds() - encoded_count.cpu_at_first
        window = window_end - first
        result.cpu = 100.0 * cpu_used / window if window > 0 else 0.0
        encoded = encoded_count.in_window(first, window_end)
        if len(encoded) > 1:
            result.fps = (len(encoded) - 1) / (encoded[-1] - encoded[0])
        result.source_fps = len(source_count.in_window(first, window_end)) / window

    if error is None:
        pipeline.send_event(Gst.Event.new_eos())
        msg = bus.timed_pop_filtered(10 * Gst.SECOND, stop_types)
        if msg is None:
            error = "no EOS within 10 s"
        elif msg.type == Gst.MessageType.ERROR:
            error = f"on EOS: {bus_error(msg)}"

    rate = pipeline.get_by_name("rate")
    result.dup = rate.get_property("duplicate")
    result.drop = rate.get_property("drop")
    result.rate_in = rate.get_property("in")
    result.rate_out = rate.get_property("out")
    pipeline.set_state(Gst.State.NULL)

    result.source_frames = len(source_count.times)
    result.frames = len(encoded_count.times)
    result.started = result.frames > 0
    result.error = error or ""
    return result


# --- ScreenCast portal (only with --source portal) --------------------------

PORTAL_NAME = "org.freedesktop.portal.Desktop"
PORTAL_PATH = "/org/freedesktop/portal/desktop"
SCREENCAST_IFACE = "org.freedesktop.portal.ScreenCast"
REQUEST_IFACE = "org.freedesktop.portal.Request"
SESSION_IFACE = "org.freedesktop.portal.Session"
SOURCE_MONITOR = 1
SOURCE_VIRTUAL = 4
PERSIST_NONE = 0
PERSIST_UNTIL_REVOKED = 2
CURSOR_EMBEDDED = 2


class Portal:
    def __init__(self):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.proxy = Gio.DBusProxy.new_sync(
            self.bus,
            Gio.DBusProxyFlags.NONE,
            None,
            PORTAL_NAME,
            PORTAL_PATH,
            SCREENCAST_IFACE,
            None,
        )
        self.sender = self.bus.get_unique_name()[1:].replace(".", "_")
        self.counter = 0
        self.session = None

    def _token(self):
        self.counter += 1
        return f"kooha_bench_{os.getpid()}_{self.counter}"

    def _request(self, method, args_format, args, options):
        """Calls a portal method that answers through a Request object."""
        token = self._token()
        options["handle_token"] = GLib.Variant("s", token)
        request_path = f"{PORTAL_PATH}/request/{self.sender}/{token}"
        loop = GLib.MainLoop()
        answer = {}

        def on_response(_conn, _sender, _path, _iface, _signal, params):
            answer["code"], answer["results"] = params.unpack()
            loop.quit()

        sub = self.bus.signal_subscribe(
            PORTAL_NAME,
            REQUEST_IFACE,
            "Response",
            request_path,
            None,
            Gio.DBusSignalFlags.NONE,
            on_response,
        )
        try:
            self.proxy.call_sync(
                method,
                GLib.Variant(args_format, (*args, options)),
                Gio.DBusCallFlags.NONE,
                -1,
                None,
            )
            loop.run()
        finally:
            self.bus.signal_unsubscribe(sub)

        if answer["code"] != 0:
            raise RuntimeError(f"{method} was cancelled (response {answer['code']})")
        return answer["results"]

    def open(self, types, persist_mode, restore_token, on_started):
        """Opens a session with one stream and returns its fd, node and size.

        `on_started(results, seconds)` runs as soon as Start answers, with
        the seconds Start took, before the PipeWire remote is opened.
        """
        results = self._request(
            "CreateSession",
            "(a{sv})",
            (),
            {"session_handle_token": GLib.Variant("s", self._token())},
        )
        self.session = results["session_handle"]

        options = {
            "types": GLib.Variant("u", types),
            "multiple": GLib.Variant("b", False),
            "persist_mode": GLib.Variant("u", persist_mode),
        }
        cursor_modes = self.proxy.get_cached_property("AvailableCursorModes")
        if cursor_modes is not None and cursor_modes.unpack() & CURSOR_EMBEDDED:
            options["cursor_mode"] = GLib.Variant("u", CURSOR_EMBEDDED)
        if restore_token:
            options["restore_token"] = GLib.Variant("s", restore_token)
        self._request("SelectSources", "(oa{sv})", (self.session,), options)

        start = time.monotonic()
        results = self._request("Start", "(osa{sv})", (self.session, ""), {})
        on_started(results, time.monotonic() - start)

        streams = results["streams"]
        if len(streams) != 1:
            raise RuntimeError(f"expected one stream, got {len(streams)}")
        node, props = streams[0]

        reply, fd_list = self.proxy.call_with_unix_fd_list_sync(
            "OpenPipeWireRemote",
            GLib.Variant("(oa{sv})", (self.session, {})),
            Gio.DBusCallFlags.NONE,
            -1,
            None,
            None,
        )
        fd = fd_list.get(reply.unpack()[0])
        return {"fd": fd, "node": node, "size": tuple(props["size"])}

    def close(self):
        if self.session is None:
            return
        self.bus.call_sync(
            PORTAL_NAME,
            self.session,
            SESSION_IFACE,
            "Close",
            None,
            None,
            Gio.DBusCallFlags.NONE,
            -1,
            None,
        )
        self.session = None


def read_restore_token():
    out = subprocess.run(
        ["gsettings", "get", KOOHA_SCHEMA, RESTORE_TOKEN_KEY],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    return GLib.Variant.parse(None, out, None, None).unpack() if out else ""


def write_restore_token(token):
    subprocess.run(
        ["gsettings", "set", KOOHA_SCHEMA, RESTORE_TOKEN_KEY, token], check=True
    )


def token_writer(write=None):
    """The on_started handler for Kooha's own session.

    Writes the successor token to Kooha's key with `write` (by default
    write_restore_token) whenever the stream is a monitor and KDE gave a
    successor, however long Start took.
    """
    write = write or write_restore_token

    def on_started(results, seconds):
        streams = results.get("streams", [])
        is_monitor = len(streams) == 1 and (
            streams[0][1].get("source_type", 0) == SOURCE_MONITOR
        )
        new_token = results.get("restore_token", "")
        print(f"Start answered in {seconds:.1f} s", flush=True)

        if not is_monitor:
            print(
                "The stream is not a monitor: Kooha's key is left as it was.",
                flush=True,
            )
        elif not new_token:
            print(
                "KDE gave no successor token: Kooha's key is left as it was, "
                "and its token is spent.",
                flush=True,
            )
        else:
            write(new_token)
            print("Wrote the successor token to Kooha's key.", flush=True)

    return on_started


def open_kooha_session(portal, args):
    token = read_restore_token()
    with open(TOKEN_BACKUP, "w") as f:
        f.write(token + "\n")
    print(f"Saved the current token to {TOKEN_BACKUP}", flush=True)
    return portal.open(
        SOURCE_MONITOR,
        PERSIST_UNTIL_REVOKED,
        token,
        token_writer(),
    )


def open_region_session(portal):
    def on_started(_results, seconds):
        print(f"Region session: Start answered in {seconds:.1f} s", flush=True)

    return portal.open(SOURCE_MONITOR | SOURCE_VIRTUAL, PERSIST_NONE, "", on_started)


def pick_monitor(monitors, stream_size):
    """The index of the monitor the stream shows, or None.

    `monitors` holds (logical width, logical height, scale) per monitor. The
    portal may report the stream's size in logical pixels (matched within 1)
    or physical ones (within 2, for the rounding of a fractional scale).
    """
    stream_w, stream_h = stream_size

    def near(size, tolerance):
        return abs(size[0] - stream_w) <= tolerance and (
            abs(size[1] - stream_h) <= tolerance
        )

    for i, (width, height, scale) in enumerate(monitors):
        physical = (round(width * scale), round(height * scale))
        if near((width, height), 1) or near(physical, 2):
            return i
    return None


def ball_window(seconds, stream_size):
    """Shows a ball bouncing inside AREA, fullscreen on the monitor the stream
    shows (pick_monitor), so the stream has something that moves.
    """
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gdk, Gtk

    Gtk.init()
    monitors = Gdk.Display.get_default().get_monitors()
    candidates = [monitors.get_item(i) for i in range(monitors.get_n_items())]
    index = pick_monitor(
        [
            (m.get_geometry().width, m.get_geometry().height, m.get_scale())
            for m in candidates
        ],
        stream_size,
    )
    monitor = None if index is None else candidates[index]
    if monitor is None:
        print(f"No monitor is {stream_size[0]}x{stream_size[1]}; no ball window")
        return
    scale = monitor.get_scale()

    # The area in logical pixels, which is what the window draws in.
    x, y, width, height = (v / scale for v in AREA)
    radius = 40 / scale
    ball = {"x": x + radius, "y": y + radius, "dx": 600 / scale, "dy": 420 / scale}
    last = {"time": None}

    def draw(_area, cr, _width, _height):
        cr.set_source_rgb(0, 0, 0)
        cr.paint()
        cr.set_source_rgb(1, 1, 1)
        cr.arc(ball["x"], ball["y"], radius, 0, 6.2832)
        cr.fill()

    def tick(widget, clock):
        now = clock.get_frame_time() / 1e6
        step = 0.0 if last["time"] is None else now - last["time"]
        last["time"] = now
        for axis, delta, low, high in (
            ("x", "dx", x + radius, x + width - radius),
            ("y", "dy", y + radius, y + height - radius),
        ):
            ball[axis] += ball[delta] * step
            if not low <= ball[axis] <= high:
                ball[delta] = -ball[delta]
                ball[axis] = min(max(ball[axis], low), high)
        widget.queue_draw()
        return True

    canvas = Gtk.DrawingArea()
    canvas.set_draw_func(draw)
    canvas.add_tick_callback(tick)
    window = Gtk.Window(title="Kooha capture bench")
    window.set_child(canvas)
    window.fullscreen_on_monitor(monitor)
    window.present()

    loop = GLib.MainLoop()
    GLib.timeout_add_seconds(int(seconds), loop.quit)
    loop.run()


# --- main --------------------------------------------------------------------


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Measure variants of Kooha's capture pipeline.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--source", choices=["test", "portal"], default="test")
    parser.add_argument(
        "--variant",
        action="append",
        choices=ALL_VARIANTS + ["all"],
        help="variant to run, repeatable (default: all)",
    )
    parser.add_argument("--seconds", type=float, default=20.0)
    parser.add_argument(
        "--fps", type=int, action="append", help="repeatable (default: 60)"
    )
    parser.add_argument("--out", help="also write the tables to this file")
    parser.add_argument("--no-ball-window", action="store_true")
    # No-ops, kept so that older command lines still run.
    parser.add_argument("--allow-dialog", action="store_true", help="ignored")
    parser.add_argument("--write-token", action="store_true", help="ignored")
    parser.add_argument("--with-kde-region", action="store_true")
    parser.add_argument("--probe-session", action="store_true")
    parser.add_argument("--ball-window", type=float, help=argparse.SUPPRESS)
    parser.add_argument("--ball-size", help=argparse.SUPPRESS)
    return parser.parse_args(argv)


def run_all(variants, source, rates, seconds, stream, lines):
    for fps in rates:
        size = f" {stream['size'][0]}x{stream['size'][1]}" if stream else ""
        header = (
            f"Source: {source}{size}, {fps} FPS, {seconds:g} s per variant, "
            f"area {AREA[2]}x{AREA[3]} at ({AREA[0]}, {AREA[1]})"
        )
        block = ["", header, "", TABLE_HEADER]
        print("\n".join(block), flush=True)
        lines.extend(block)
        discarded = None
        for variant in variants:
            result = run_variant(variant, source, fps, seconds, stream)
            print(result.row(), flush=True)
            lines.append(result.row())
            if variant == "today" and result.source_fps < MIN_SOURCE_SHARE * fps:
                discarded = (
                    f"**Discarded pass:** the source delivered {result.source_fps:.1f} "
                    f"of {fps} FPS in `today` (under {MIN_SOURCE_SHARE:.0%})."
                )
        if discarded:
            print(f"\n{discarded}", flush=True)
            lines.extend(["", discarded])


def start_ball_window(seconds, stream_size):
    size = f"{stream_size[0]}x{stream_size[1]}"
    ball = subprocess.Popen(
        [
            sys.executable,
            "-I",
            __file__,
            "--ball-window",
            str(seconds),
            "--ball-size",
            size,
        ]
    )
    time.sleep(2)
    return ball


def stop_ball_window(ball):
    if ball is not None:
        ball.terminate()
        ball.wait()


def main(argv):
    args = parse_args(argv)
    Gst.init(None)

    if args.ball_window is not None:
        width, height = (int(v) for v in args.ball_size.split("x"))
        ball_window(args.ball_window, (width, height))
        return 0

    if args.probe_session:
        portal = Portal()
        try:
            stream = open_kooha_session(portal, args)
            print(f"Session open: node {stream['node']}, size {stream['size']}")
            os.close(stream["fd"])
        finally:
            portal.close()
        return 0

    variants = args.variant or ["all"]
    if "all" in variants:
        variants = ALL_VARIANTS
    if args.source == "test":
        variants = [v for v in variants if v not in PORTAL_ONLY]
    rates = args.fps or [60]
    lines = []

    if args.source == "test":
        run_all(variants, "test", rates, args.seconds, None, lines)
    else:
        runs = len(variants) * len(rates) + (len(rates) if args.with_kde_region else 0)
        ball_seconds = runs * (args.seconds + STARTUP_TIMEOUT + 10) + 60
        ball = None
        try:
            portal = Portal()
            try:
                stream = open_kooha_session(portal, args)
                if not args.no_ball_window:
                    ball = start_ball_window(ball_seconds, stream["size"])
                run_all(variants, "portal", rates, args.seconds, stream, lines)
            finally:
                portal.close()

            if args.with_kde_region:
                region_portal = Portal()
                try:
                    stream = open_region_session(region_portal)
                    run_all([KDE_REGION], "portal", rates, args.seconds, stream, lines)
                finally:
                    region_portal.close()
        finally:
            stop_ball_window(ball)

    if args.out:
        with open(args.out, "w") as f:
            f.write("\n".join(lines).lstrip("\n") + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
