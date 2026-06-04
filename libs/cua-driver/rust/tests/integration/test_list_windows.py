"""Integration test: `list_windows` tool.

Exercises three contracts against the real driver over stdio MCP:

  1. Default call returns at least a handful of windows across multiple
     pids, every record layer-0 with the correct field names.
  2. `pid` filter narrows the result to that pid only.
  3. `on_screen_only: true` drops hidden / minimized / off-Space windows.

Calculator is launched once (setUpClass) so all five tests share one
driver session — avoids the 13-second Calculator restart per-test.

Run:
    CUA_DRIVER_BINARY=../../target/debug/cua-driver python3 -m unittest test_list_windows -v
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path


CALCULATOR_BUNDLE = "com.apple.calculator"


class ListWindowsTests(unittest.TestCase):
    client: DriverClient
    calc_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)
        cls.client = DriverClient(default_binary_path()).__enter__()
        result = cls.client.call_tool("launch_app", {"bundle_id": CALCULATOR_BUNDLE})
        cls.calc_pid = result["structuredContent"]["pid"]
        time.sleep(0.5)

    @classmethod
    def tearDownClass(cls) -> None:
        cls.client.__exit__(None, None, None)
        subprocess.run(["pkill", "-x", "Calculator"], check=False)

    def _call_list_windows(self, **args):
        return self.client.call_tool("list_windows", args)["structuredContent"]

    def test_default_returns_layer_zero_windows_with_field_names(self):
        body = self._call_list_windows()
        windows = body["windows"]
        self.assertGreater(
            len(windows), 0, "expected at least Calculator's window"
        )

        required = {
            "window_id", "pid", "app_name", "title", "bounds",
            "layer", "z_index", "is_on_screen",
        }
        for w in windows:
            self.assertTrue(
                required.issubset(w.keys()),
                f"window record missing fields: {required - set(w.keys())}",
            )
            self.assertEqual(w["layer"], 0)

    def test_pid_filter_narrows_to_one_pid(self):
        body = self._call_list_windows(pid=self.calc_pid)
        windows = body["windows"]
        self.assertGreater(
            len(windows), 0, "Calculator should have at least one window"
        )
        self.assertTrue(
            all(w["pid"] == self.calc_pid for w in windows),
            "pid filter leaked other pids into the result",
        )
        self.assertTrue(
            all(w["app_name"] == "Calculator" for w in windows),
            "app_name disagrees with the pid filter",
        )

    def test_on_screen_only_drops_off_screen_entries(self):
        everything = self._call_list_windows()["windows"]
        only_visible = self._call_list_windows(on_screen_only=True)["windows"]

        self.assertLessEqual(len(only_visible), len(everything))
        for w in only_visible:
            self.assertTrue(
                w["is_on_screen"],
                "on_screen_only=true returned a window with is_on_screen=false",
            )

    def test_bounds_fields_present_and_non_negative(self):
        body = self._call_list_windows(pid=self.calc_pid)
        windows = body["windows"]
        self.assertGreater(len(windows), 0)
        for w in windows:
            b = w["bounds"]
            for field in ("x", "y", "width", "height"):
                self.assertIn(field, b, f"bounds missing '{field}'")
                self.assertGreaterEqual(
                    b[field], 0,
                    f"bounds.{field} should be non-negative, got {b[field]}",
                )

    def test_z_index_positive(self):
        body = self._call_list_windows()
        windows = body["windows"]
        for w in windows:
            self.assertGreater(
                w["z_index"], 0,
                f"z_index should be > 0 (front-to-back ordering), got {w['z_index']}",
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
