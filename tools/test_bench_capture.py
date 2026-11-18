#!/usr/bin/python3
"""Tests for tools/bench-capture.py.

Run from the repository root with the system Python:

    /usr/bin/python3 -I tools/test_bench_capture.py

They never open the ScreenCast portal, touch gsettings or show a window: the
portal answers and the monitors are injected, and the pipelines use
videotestsrc.
"""

import contextlib
import importlib.util
import io
import os
import sys
import tempfile
import time
import unittest

# Keeps tools/ free of a __pycache__ for the imported script.
sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location(
    "bench_capture", os.path.join(HERE, "bench-capture.py")
)
bench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench)
bench.Gst.init(None)


def start_results(source_type=bench.SOURCE_MONITOR, token="successor"):
    """What Start answers, as Portal._request unpacks it."""
    results = {"streams": [(42, {"source_type": source_type, "size": (1680, 1050)})]}
    if token:
        results["restore_token"] = token
    return results


class TokenWriterTest(unittest.TestCase):
    def setUp(self):
        self.written = []
        self.on_started = bench.token_writer(self.written.append)

    def test_slow_silent_start_writes_the_successor(self):
        # A restore without a dialog may take over two seconds; the original
        # token is spent either way, so only the successor keeps it working.
        self.on_started(start_results(), 2.1)

        self.assertEqual(self.written, ["successor"])

    def test_fast_start_writes_the_successor(self):
        self.on_started(start_results(), 0.3)

        self.assertEqual(self.written, ["successor"])

    def test_stream_that_is_not_a_monitor_is_not_written(self):
        self.on_started(start_results(source_type=bench.SOURCE_VIRTUAL), 0.3)

        self.assertEqual(self.written, [])

    def test_missing_successor_is_not_written(self):
        self.on_started(start_results(token=""), 0.3)

        self.assertEqual(self.written, [])


class TokenBackupTest(unittest.TestCase):
    NOW = time.struct_time((2026, 11, 18, 21, 5, 9, 2, 322, 0))

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="kooha-bench-test-")
        self.dir = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def backup(self, token):
        return bench.backup_token(token, runtime_dir=self.dir, now=self.NOW)

    def test_backup_goes_to_the_runtime_dir_with_the_date(self):
        path = self.backup("original")

        self.assertEqual(
            path, os.path.join(self.dir, "kooha-bench-token-20261118-210509.bak")
        )
        with open(path) as f:
            self.assertEqual(f.read(), "original\n")

    def test_backup_is_private(self):
        path = self.backup("original")

        self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)

    def test_second_backup_never_overwrites_the_first(self):
        first = self.backup("original")
        second = self.backup("successor")

        self.assertNotEqual(first, second)
        with open(first) as f:
            self.assertEqual(f.read(), "original\n")
        with open(second) as f:
            self.assertEqual(f.read(), "successor\n")

    def test_backup_does_not_follow_a_planted_symlink(self):
        target = os.path.join(self.dir, "target")
        planted = os.path.join(self.dir, "kooha-bench-token-20261118-210509.bak")
        os.symlink(target, planted)

        path = self.backup("original")

        self.assertNotEqual(path, planted)
        self.assertFalse(os.path.exists(target))

    def test_no_runtime_dir_is_an_error(self):
        with self.assertRaises(RuntimeError):
            bench.backup_token("original", runtime_dir="", now=self.NOW)


SESSION = "/org/freedesktop/portal/desktop/session/1_2/kooha_bench"


class FakeBus:
    """The session bus, as Portal uses it: it answers every request but the
    ones in `silent`, and records the session closes."""

    def __init__(self, silent=(), output=None):
        self.silent = silent
        self.output = output
        self.handlers = {}
        self.calls = []
        self.closed = []
        self.output_at = {}

    def get_unique_name(self):
        return ":1.2"

    def signal_subscribe(self, _name, _iface, _signal, path, *rest):
        handler = rest[-1]
        self.handlers[path] = handler
        return path

    def signal_unsubscribe(self, sub):
        self.handlers.pop(sub, None)

    def answer(self, method, request_path):
        self.calls.append(method)
        if self.output is not None:
            self.output_at[method] = self.output.getvalue()
        if method in self.silent:
            return
        results = {
            "CreateSession": {"session_handle": bench.GLib.Variant("s", SESSION)},
            "SelectSources": {},
            "Start": {
                "streams": bench.GLib.Variant(
                    "a(ua{sv})",
                    [
                        (
                            42,
                            {
                                "source_type": bench.GLib.Variant("u", 1),
                                "size": bench.GLib.Variant("(ii)", (1680, 1050)),
                            },
                        )
                    ],
                ),
                "restore_token": bench.GLib.Variant("s", "successor"),
            },
        }[method]
        params = bench.GLib.Variant("(ua{sv})", (0, results))
        self.handlers[request_path](None, None, request_path, None, None, params)

    def call_sync(self, _name, path, _iface, method, *_rest):
        self.closed.append((path, method))


class FakeProxy:
    def __init__(self, bus):
        self.bus = bus

    def get_cached_property(self, _name):
        return None

    def call_sync(self, method, args, *_rest):
        options = args.unpack()[-1]
        token = options["handle_token"]
        self.bus.answer(method, f"{bench.PORTAL_PATH}/request/1_2/{token}")


def fake_portal(bus, start_timeout=bench.START_TIMEOUT):
    portal = bench.Portal.__new__(bench.Portal)
    portal.bus = bus
    portal.proxy = FakeProxy(bus)
    portal.sender = "1_2"
    portal.counter = 0
    portal.session = None
    portal.start_timeout = start_timeout
    return portal


class StartTest(unittest.TestCase):
    def test_start_has_a_thirty_second_deadline(self):
        self.assertEqual(bench.START_TIMEOUT, 30)
        self.assertEqual(bench.Portal.start_timeout, 30)

    def test_unanswered_start_closes_the_session_without_writing(self):
        bus = FakeBus(silent={"Start"})
        portal = fake_portal(bus, start_timeout=0.3)
        written = []

        start = time.monotonic()
        with self.assertRaises(bench.StartTimeout):
            portal.open(
                bench.SOURCE_MONITOR,
                bench.PERSIST_UNTIL_REVOKED,
                "original",
                bench.token_writer(written.append),
                "note",
            )
        elapsed = time.monotonic() - start

        self.assertGreaterEqual(elapsed, 0.3)
        self.assertLess(elapsed, 2)
        self.assertEqual(written, [])
        self.assertEqual(bus.closed, [(SESSION, "Close")])
        self.assertIsNone(portal.session)

    def test_answered_start_writes_the_successor(self):
        bus = FakeBus(silent={"OpenPipeWireRemote"})
        portal = fake_portal(bus)
        written = []
        portal.proxy.call_with_unix_fd_list_sync = lambda *_a: (
            bench.GLib.Variant("(h)", (0,)),
            FakeFdList(),
        )

        stream = portal.open(
            bench.SOURCE_MONITOR,
            bench.PERSIST_UNTIL_REVOKED,
            "original",
            bench.token_writer(written.append),
            "note",
        )

        self.assertEqual(written, ["successor"])
        self.assertEqual(stream["size"], (1680, 1050))

    def test_each_start_says_first_whether_to_answer_the_dialog(self):
        for opener, expected in [
            (bench.open_kooha_session, "Kooha session: do not answer a dialog"),
            (bench.open_region_session, "Region session: pick the region now"),
        ]:
            output = io.StringIO()
            bus = FakeBus(silent={"Start"}, output=output)
            portal = fake_portal(bus, start_timeout=0.05)
            with contextlib.redirect_stdout(output), self.assertRaises(
                bench.StartTimeout
            ):
                if opener is bench.open_kooha_session:
                    opener(portal, None, read=lambda: "original", backup=lambda _t: "/x")
                else:
                    opener(portal)

            self.assertIn(expected, bus.output_at["Start"], opener.__name__)
            self.assertNotIn(expected, bus.output_at["SelectSources"], opener.__name__)


class FakeFdList:
    def get(self, _index):
        return 7


# DP-3 at 105 %: KDE may report the logical size, the frames are physical.
DP3 = (1600, 1000, 1.05)
EDP1 = (1463, 915, 1.75)  # 2560x1601 physical


class PickMonitorTest(unittest.TestCase):
    def test_logical_size_picks_the_monitor(self):
        self.assertEqual(bench.pick_monitor([EDP1, DP3], (1600, 1000)), 1)

    def test_physical_size_picks_the_monitor(self):
        self.assertEqual(bench.pick_monitor([EDP1, DP3], (1680, 1050)), 1)

    def test_logical_size_one_pixel_off_picks_the_monitor(self):
        self.assertEqual(bench.pick_monitor([EDP1, DP3], (1601, 999)), 1)

    def test_physical_size_two_pixels_off_picks_the_monitor(self):
        self.assertEqual(bench.pick_monitor([DP3, EDP1], (2560, 1600)), 1)

    def test_no_monitor_of_that_size(self):
        self.assertIsNone(bench.pick_monitor([EDP1, DP3], (1920, 1080)))


def caps_size(pad):
    caps = pad.get_current_caps()
    if caps is None:
        return None
    s = caps.get_structure(0)
    return s.get_value("width"), s.get_value("height")


def negotiated_sizes(pipeline, timeout=10):
    """Plays `pipeline` until the encoder output has caps. Returns the size of
    the frames into the crop and out of the encoder.
    """
    bus = pipeline.get_bus()
    crop_in = pipeline.get_by_name("crop").get_static_pad("sink")
    encoded = pipeline.get_by_name("encoded").get_static_pad("src")
    pipeline.set_state(bench.Gst.State.PLAYING)
    try:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if caps_size(encoded) is not None:
                return caps_size(crop_in), caps_size(encoded)
            msg = bus.timed_pop_filtered(
                50 * bench.Gst.MSECOND, bench.Gst.MessageType.ERROR
            )
            if msg is not None:
                raise AssertionError(bench.bus_error(msg))
        raise AssertionError(f"no caps on the encoder output within {timeout} s")
    finally:
        pipeline.set_state(bench.Gst.State.NULL)


class CropTest(unittest.TestCase):
    """The crop comes from the negotiated frames, not from the portal's size."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory(prefix="kooha-bench-test-")
        self.out = os.path.join(self.dir.name, "out.mp4")
        # The portal said 1600x1000 (logical); the frames are 1680x1050.
        self.stream = {"size": (1600, 1000)}

    def tearDown(self):
        self.dir.cleanup()

    def assert_area_size(self, variant):
        pipeline = bench.build_pipeline(
            variant, "test", 30, self.stream, test_size=(1680, 1050), out=self.out
        )
        # The whole frame, unscaled, reaches the crop, and the area comes out.
        self.assertEqual(negotiated_sizes(pipeline), ((1680, 1050), (1280, 720)))

    def test_today(self):
        self.assert_area_size("today")

    def test_crop_first(self):
        self.assert_area_size("crop-first")

    def test_crop_first_queues(self):
        self.assert_area_size("crop-first-queues")


PERIOD = bench.Gst.SECOND // 60


def pts_without(missing, total=1200):
    """The PTS of `total` frames at 60 FPS, minus the frames in `missing`."""
    return [i * PERIOD for i in range(total) if i not in missing]


class MissingFramesTest(unittest.TestCase):
    def test_steady_source_misses_nothing(self):
        self.assertEqual(bench.missing_frames(pts_without(set()), PERIOD), 0)

    def test_one_long_gap(self):
        missing = set(range(500, 540))

        self.assertEqual(bench.missing_frames(pts_without(missing), PERIOD), 40)

    def test_scattered_gaps(self):
        missing = {30 * i + 7 for i in range(40)}

        self.assertEqual(bench.missing_frames(pts_without(missing), PERIOD), 40)

    def test_jitter_under_one_and_a_half_periods_is_no_gap(self):
        pts = [0, PERIOD, int(2.5 * PERIOD), int(3.5 * PERIOD)]

        self.assertEqual(bench.missing_frames(pts, PERIOD), 0)

    def test_gap_rounds_to_the_nearest_frame_count(self):
        # 1.6 periods: one frame missing; 3.4 periods: two.
        pts = [0, int(1.6 * PERIOD), int(5.0 * PERIOD)]

        self.assertEqual(bench.missing_frames(pts, PERIOD), 3)

    def test_frames_without_pts_are_left_out(self):
        pts = [0, bench.Gst.CLOCK_TIME_NONE, PERIOD, 2 * PERIOD]

        self.assertEqual(bench.missing_frames(pts, PERIOD), 0)


def result(missing=0, queue_drop=0, requested=1200):
    """A pass at 60 FPS where videorate fills every hole up to `requested`."""
    r = bench.Result("today")
    r.source_frames = requested - missing
    r.missing = missing
    r.rate_in = r.source_frames - queue_drop
    r.dup = missing + queue_drop
    r.rate_out = requested
    r.frames = requested
    return r


class ResultTest(unittest.TestCase):
    def test_duplicates_do_not_hide_frames_missing_at_the_source(self):
        r = result(missing=40)

        self.assertEqual(r.frames, 1200)
        self.assertEqual(r.queue_drop, 0)
        self.assertFalse(r.ok)

    def test_queue_drops_count(self):
        self.assertFalse(result(queue_drop=13).ok)

    def test_missing_and_dropped_add_up(self):
        self.assertFalse(result(missing=7, queue_drop=6).ok)

    def test_under_one_percent_is_ok(self):
        self.assertTrue(result(missing=6, queue_drop=5).ok)

    def test_exactly_one_percent_is_not_ok(self):
        self.assertFalse(result(missing=6, queue_drop=6).ok)

    def test_over_one_percent_is_not_ok(self):
        self.assertFalse(result(missing=7, queue_drop=6).ok)

    def test_nothing_recorded_is_not_ok(self):
        self.assertFalse(bench.Result("today").ok)

    def test_row_separates_the_counts(self):
        cells = [c.strip() for c in result(missing=40, queue_drop=3).row().split("|")]
        header = [c.strip() for c in bench.TABLE_HEADER.splitlines()[0].split("|")]
        row = dict(zip(header, cells))

        self.assertEqual(row["source frames"], "1160")
        self.assertEqual(row["missing"], "40")
        self.assertEqual(row["dup"], "43")
        self.assertEqual(row["queue drop"], "3")
        self.assertEqual(row["ok <1%"], "no")


class RunVariantTest(unittest.TestCase):
    def test_steady_test_source_misses_nothing(self):
        with tempfile.TemporaryDirectory(prefix="kooha-bench-test-") as tmp:
            r = bench.run_variant(
                "crop-first-queues",
                "test",
                30,
                1,
                None,
                out=os.path.join(tmp, "out.mp4"),
            )

        self.assertEqual(r.error, "")
        self.assertGreater(r.source_frames, 25)
        self.assertEqual(r.missing, 0)
        self.assertEqual(r.queue_drop, 0)
        self.assertTrue(r.ok)


if __name__ == "__main__":
    unittest.main()
