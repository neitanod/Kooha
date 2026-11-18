#!/usr/bin/python3
"""Tests for tools/bench-capture.py.

Run from the repository root with the system Python:

    /usr/bin/python3 -I tools/test_bench_capture.py

They never open the ScreenCast portal, touch gsettings or show a window: the
portal answers and the monitors are injected, and the pipelines use
videotestsrc.
"""

import importlib.util
import os
import tempfile
import time
import unittest

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


if __name__ == "__main__":
    unittest.main()
