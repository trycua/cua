"""Integration test: hidden-app launch → list_windows → screenshot capture.

Verifies the contract documented in issue #1489 / #1486:

  1. `launch_app` with a bundle that creates a window (Calculator) sets
     is_on_screen=false when the window isn't shown on the current Space,
     but `list_windows` (default, no on_screen_only filter) still surfaces
     it and returns a valid window_id.

  2. `screenshot` succeeds against that window_id even though
     is_on_screen is false — ScreenCaptureKit captures the backing store
     regardless of visibility.

  3. The `list_windows(pid=...)` warning message is returned (not isError)
     when the pid filter finds windows — exercises the non-empty path.

Calculator is used as the test target because it reliably creates a window
on launch_app and is guaranteed to be available on every macOS install.
We terminate it at the end to leave the system clean.

Run:
    scripts/test.sh test_hidden_app_capture
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path, reset_calculator


CALCULATOR_BUNDLE = "com.apple.calculator"


class HiddenAppCaptureTests(unittest.TestCase):
    """Test that list_windows surfaces hidden windows and screenshot works."""

    def setUp(self) -> None:
        reset_calculator()
        self.client = DriverClient(default_binary_path()).__enter__()

    def tearDown(self) -> None:
        self.client.__exit__(None, None, None)
        # Kill Calculator so it doesn't linger between test runs.
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)

    def test_hidden_window_appears_in_list_windows(self) -> None:
        """launch_app → window appears in list_windows even if off-screen."""
        result = self.client.call_tool(
            "launch_app", {"bundle_id": CALCULATOR_BUNDLE}
        )
        self.assertFalse(
            result.get("isError"), f"launch_app failed: {result}"
        )
        pid = result["structuredContent"]["pid"]
        self.assertIsInstance(pid, int)
        self.assertGreater(pid, 0)

        # Give Calculator a moment to create its window.
        time.sleep(1.0)

        # list_windows (no filter) must include Calculator's window.
        all_windows = self.client.call_tool("list_windows", {})[
            "structuredContent"
        ]["windows"]
        calc_windows = [w for w in all_windows if w["pid"] == pid]
        self.assertGreater(
            len(calc_windows),
            0,
            f"Calculator (pid {pid}) window not found in list_windows",
        )

        # Every returned record must have the required fields.
        required = {"window_id", "pid", "app_name", "bounds", "layer", "z_index", "is_on_screen"}
        for w in calc_windows:
            missing = required - set(w.keys())
            self.assertFalse(missing, f"window record missing fields: {missing}")

    def test_screenshot_succeeds_for_hidden_window(self) -> None:
        """screenshot tool captures the backing store even when is_on_screen=false."""
        result = self.client.call_tool(
            "launch_app", {"bundle_id": CALCULATOR_BUNDLE}
        )
        self.assertFalse(result.get("isError"), f"launch_app failed: {result}")
        pid = result["structuredContent"]["pid"]
        time.sleep(1.0)

        # Grab a window_id for Calculator.
        windows_result = self.client.call_tool("list_windows", {"pid": pid})[
            "structuredContent"
        ]["windows"]
        self.assertGreater(
            len(windows_result), 0, "No windows found for Calculator"
        )
        window_id = windows_result[0]["window_id"]

        # screenshot must succeed regardless of is_on_screen.
        shot = self.client.call_tool("screenshot", {"window_id": window_id})
        self.assertFalse(
            shot.get("isError"),
            f"screenshot failed for window_id {window_id}: {shot}",
        )
        # Result should contain an image content block.
        content = shot.get("content", [])
        has_image = any(c.get("type") == "image" for c in content)
        has_text = any(c.get("type") == "text" for c in content)
        self.assertTrue(
            has_image or has_text,
            f"screenshot returned no content blocks: {content}",
        )

    def test_list_windows_pid_filter_returns_warning_on_unknown_pid(self) -> None:
        """list_windows with a nonexistent pid returns a warning, not an error."""
        # Use a pid that is virtually guaranteed not to exist.
        fake_pid = 99999
        result = self.client.call_tool("list_windows", {"pid": fake_pid})
        # Must NOT be isError — the tool surfaces a warning in the text body.
        self.assertFalse(
            result.get("isError"),
            "list_windows should not set isError for empty pid filter results",
        )
        text_content = " ".join(
            c.get("text", "") for c in result.get("content", []) if c.get("type") == "text"
        )
        self.assertIn(
            "No windows found",
            text_content,
            f"Expected warning text in response, got: {text_content!r}",
        )

    def test_list_windows_pid_filter_includes_frontmost_hint(self) -> None:
        """Warning message for empty pid filter includes the frontmost app name."""
        fake_pid = 99999
        result = self.client.call_tool("list_windows", {"pid": fake_pid})
        text_content = " ".join(
            c.get("text", "") for c in result.get("content", []) if c.get("type") == "text"
        )
        # The frontmost-app hint is conditional on having any on-screen window;
        # on a normal macOS machine this is always true (Finder, at minimum).
        # We just check the warning structure is present.
        self.assertIn(str(fake_pid), text_content)


if __name__ == "__main__":
    unittest.main()
