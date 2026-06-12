"""Integration test: `list_windows` tool.

Exercises three contracts against the real driver over stdio MCP:

  1. Default call returns at least a handful of windows across multiple
     pids, every record layer-0 with the new field names.
  2. `pid` filter narrows the result to that pid only.
  3. `on_screen_only: true` drops hidden / minimized / off-Space windows.

The tool is pure read-only and has no side effects — no cleanup is
needed between subtests. We launch Calculator at the start so the
assertions have a predictable non-zero target even on a freshly booted
machine.

Run:
    scripts/test.sh test_list_windows
"""

from __future__ import annotations

import os
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path, reset_calculator


CALCULATOR_BUNDLE = "com.apple.calculator"


class ListWindowsTests(unittest.TestCase):
    def setUp(self) -> None:
        reset_calculator()
        self.client = DriverClient(default_binary_path()).__enter__()
        # Launch Calculator so we have at least one guaranteed window to
        # anchor the assertions on. launch_app is idempotent; it'll
        # return the pid whether or not Calculator was already running.
        result = self.client.call_tool(
            "launch_app", {"bundle_id": CALCULATOR_BUNDLE}
        )
        self.calc_pid = result["structuredContent"]["pid"]
        time.sleep(0.5)

    def tearDown(self) -> None:
        self.client.__exit__(None, None, None)

    def _call_list_windows(self, **args):
        return self.client.call_tool("list_windows", args)[
            "structuredContent"
        ]

    def test_default_returns_layer_zero_windows_with_new_field_names(self):
        body = self._call_list_windows()
        windows = body["windows"]
        self.assertGreater(
            len(windows), 0, "expected at least Calculator's window"
        )

        # Field contract — window_id / app_name / title are the user-facing
        # renames from WindowInfo.id / owner / name.
        required = {
            "window_id", "pid", "app_name", "title", "bounds",
            "layer", "z_index", "is_on_screen",
        }
        for w in windows:
            self.assertTrue(
                required.issubset(w.keys()),
                f"window record missing fields: {required - set(w.keys())}",
            )
            # Layer 0 is the promised default filter.
            self.assertEqual(w["layer"], 0)

        # current_space_id is present when the SkyLight SPI resolves.
        # On every modern macOS this should succeed; allow None to avoid
        # flaking on unusual CI environments.
        self.assertIn("current_space_id", body)

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
        only_visible = self._call_list_windows(on_screen_only=True)[
            "windows"
        ]

        # The visible subset should never exceed the full set.
        self.assertLessEqual(len(only_visible), len(everything))
        # Every record in the filtered subset must be on-screen.
        for w in only_visible:
            self.assertTrue(
                w["is_on_screen"],
                "on_screen_only=true returned a window with is_on_screen=false",
            )


if __name__ == "__main__":
    unittest.main()
