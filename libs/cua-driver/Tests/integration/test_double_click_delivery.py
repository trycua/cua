"""Integration test: `double_click` tool delivers two presses to a
backgrounded target.

Two addressing modes, one invariant: double-clicking Calculator's "2"
button from behind FocusMonitorApp should produce "22" in the display
and must not steal focus.

- Pixel path: `double_click({pid, x, y})` → primer-gated auth-signed
  recipe, two target down/up pairs with clickState 1→2.
- Element-index path on an AXButton (no AXOpen advertised) → falls
  through to the pixel recipe at the element's on-screen center.
  Verifies the AXOpen-fallback branch of `DoubleClickTool`.

Shares FocusMonitorApp scaffolding with `test_pixel_click_delivery.py`.

Run:
    scripts/test.sh test_double_click_delivery
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import (
    DriverClient,
    default_binary_path,
    frontmost_bundle_id,
    resolve_window_id,
)

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))
_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_BUNDLE = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.app")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_BUNDLE, "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"

CALC_BUNDLE = "com.apple.calculator"
FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"

# Calculator standard-mode (230x408) button grid — "2" is the middle
# column of the bottom-but-one row. Matches the coords used in
# test_pixel_click_delivery.py so the two tests can't drift.
CALC_TWO_BUTTON_XY = (86, 313)
CALC_DEFAULT_W = 230
CALC_DEFAULT_H = 408


def _build_focus_app() -> None:
    if not os.path.exists(_FOCUS_APP_EXE):
        subprocess.run(
            [os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True
        )


def _launch_focus_app() -> tuple[subprocess.Popen, int]:
    proc = subprocess.Popen(
        [_FOCUS_APP_EXE],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    for _ in range(40):
        line = proc.stdout.readline().strip()
        if line.startswith("FOCUS_PID="):
            return proc, int(line.split("=", 1)[1])
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError("FocusMonitorApp did not print FOCUS_PID in time")


def _read_focus_losses() -> int:
    try:
        with open(_LOSS_FILE) as f:
            return int(f.read().strip())
    except (FileNotFoundError, ValueError):
        return -1


def _find_calc_button(tree: str, label: str) -> int | None:
    for line in tree.split("\n"):
        if "AXButton" not in line:
            continue
        m = re.search(r"\[(\d+)\]", line)
        if not m:
            continue
        if f"({label})" in line or f"id={label}" in line:
            return int(m.group(1))
    return None


def _calc_display(client: DriverClient, pid: int) -> str:
    window_id = resolve_window_id(client, pid)
    result = client.call_tool(
        "get_window_state",
        {"pid": pid, "window_id": window_id, "query": "AXStaticText"},
    )
    tree = result.get("structuredContent", result).get("tree_markdown", "")
    for line in tree.split("\n"):
        if "AXStaticText" in line and "=" in line:
            m = re.search(r'= "([^"]*)"', line)
            if m:
                return m.group(1).replace("‎", "").replace("‏", "")
    return ""


class DoubleClickDeliveryTests(unittest.TestCase):
    """double_click on backgrounded Calculator — display shows '22', no focus steal."""

    _calc_pid: int
    _focus_proc: subprocess.Popen
    _focus_pid: int
    binary: str

    @classmethod
    def setUpClass(cls) -> None:
        _build_focus_app()
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)

        cls.binary = default_binary_path()

        with DriverClient(cls.binary) as c:
            result = c.call_tool("launch_app", {"bundle_id": CALC_BUNDLE})
            cls._calc_pid = result["structuredContent"]["pid"]
        print(f"\n  Calculator pid: {cls._calc_pid}")
        time.sleep(1.5)

        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        print(f"  FocusMonitor pid: {cls._focus_pid}")
        time.sleep(1.0)
        # Activate FocusMonitorApp explicitly — without this, running
        # the test on a dev machine with another app already frontmost
        # (Slack, browser, etc.) fails setUpClass even though the
        # feature under test is fine.
        subprocess.run(
            ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
            check=False,
        )
        time.sleep(0.8)

        with DriverClient(cls.binary) as c:
            active = frontmost_bundle_id(c)
            assert active == FOCUS_MONITOR_BUNDLE, (
                f"Expected FocusMonitorApp frontmost, got {active}"
            )
        losses = _read_focus_losses()
        assert losses == 0, f"Expected 0 focus losses at start, got {losses}"

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        try:
            cls._focus_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            cls._focus_proc.kill()
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    def setUp(self) -> None:
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._calc_pid, "window_id": window_id},
            )
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn = _find_calc_button(tree, "All Clear") or _find_calc_button(
                tree, "Clear"
            )
            if btn is not None:
                c.call_tool(
                    "click",
                    {
                        "pid": self._calc_pid,
                        "window_id": window_id,
                        "element_index": btn,
                    },
                )
                time.sleep(0.3)
        subprocess.run(
            ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
            check=False,
        )
        time.sleep(0.5)
        self._losses_before = _read_focus_losses()

    def _assert_display_22_no_focus_steal(self) -> None:
        time.sleep(0.3)
        with DriverClient(self.binary) as c:
            display = _calc_display(c, self._calc_pid)
        print(f"  display: '{display}'")
        self.assertEqual(
            display, "22",
            f"Double-click did not land two presses — display is '{display}'"
        )

        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        print(f"  losses: {self._losses_before}->{losses}, frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen — frontmost is {active}",
        )

    def test_pixel_double_click_delivers_two_presses(self) -> None:
        """double_click({x, y}) on backgrounded Calculator's "2" → '22'."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._calc_pid, "window_id": window_id},
            )
            sc = snap.get("structuredContent", snap)
            w = sc.get("screenshot_width", CALC_DEFAULT_W)
            h = sc.get("screenshot_height", CALC_DEFAULT_H)

            bx, by = CALC_TWO_BUTTON_XY
            x = int(bx * w / CALC_DEFAULT_W)
            y = int(by * h / CALC_DEFAULT_H)
            result = c.call_tool(
                "double_click",
                {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "x": x,
                    "y": y,
                },
            )
            sr = result.get("structuredContent", result)
            print(
                f"  pixel ({x},{y}) -> screen ({sr.get('screen_x')},{sr.get('screen_y')})"
            )
            self.assertIsNone(result.get("isError"), msg=result)

        self._assert_display_22_no_focus_steal()

    def test_element_index_double_click_falls_back_to_pixel_recipe(self) -> None:
        """double_click({element_index: <"2" button>}) — button doesn't advertise
        AXOpen, so DoubleClickTool falls through to the pixel recipe at the
        element's on-screen center. Same observable: display shows '22'."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._calc_pid, "window_id": window_id},
            )
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            two_button = _find_calc_button(tree, "2")
            self.assertIsNotNone(
                two_button,
                "Could not locate Calculator's '2' AXButton in the tree",
            )

            result = c.call_tool(
                "double_click",
                {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "element_index": two_button,
                },
            )
            self.assertIsNone(result.get("isError"), msg=result)
            summary = result.get("content", [{}])[0].get("text", "")
            print(f"  summary: {summary}")
            # The fallback branch says "Posted double-click to [N] ..."; the
            # AXOpen branch says "Performed AXOpen on [N] ...". A Calculator
            # button should never hit AXOpen — assert on the verb to catch
            # regressions where the advertised-actions check flips.
            self.assertIn(
                "double-click",
                summary.lower(),
                f"Expected fallback-to-pixel summary, got: {summary}",
            )

        self._assert_display_22_no_focus_steal()


if __name__ == "__main__":
    unittest.main(verbosity=2)
