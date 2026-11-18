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
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location(
    "bench_capture", os.path.join(HERE, "bench-capture.py")
)
bench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench)


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


if __name__ == "__main__":
    unittest.main()
