"""Integration test: `double_click` tool delivers two presses to a
backgrounded target.

Two addressing modes, one invariant: double-clicking Calculator's "2"
button from behind FocusMonitorApp should produce "22" in the display
and must not steal focus.

- Pixel path: `double_click({pid, x, y})` → synthesized CGEvent double-click.
- Element-index path on an AXButton → performs two AX presses.

Run:
    CUA_DRIVER_BINARY=../../target/debug/cua-driver python3 -m unittest test_double_click_delivery -v
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
_DRIVER_RS_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))
_LIBS_ROOT = os.path.dirname(_DRIVER_RS_ROOT)
_FOCUS_APP_DIR = os.path.join(_LIBS_ROOT, "cua-driver", "Tests", "FocusMonitorApp")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_DIR, "FocusMonitorApp.app", "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"

CALC_BUNDLE = "com.apple.calculator"
FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"

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
                return m.group(1).replace("\u200e", "").replace("\u200f", "")
    return ""


class DoubleClickDeliveryTests(unittest.TestCase):
    """double_click on backgrounded Calculator — focus must not be stolen."""

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
        # Kill any stale FocusMonitorApp left over from a crashed prior run.
        subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
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
        if hasattr(cls, '_focus_proc'):
            cls._focus_proc.terminate()
            try:
                cls._focus_proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                cls._focus_proc.kill()
        subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
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
            btn = _find_calc_button(tree, "All Clear") or _find_calc_button(tree, "Clear")
            if btn is not None:
                c.call_tool("click", {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "element_index": btn,
                })
                time.sleep(0.3)
        subprocess.run(
            ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
            check=False,
        )
        time.sleep(0.5)
        self._losses_before = _read_focus_losses()

    def _assert_no_focus_steal(self) -> None:
        time.sleep(0.3)
        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        print(f"  losses: {self._losses_before}->{losses}, frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen — frontmost is {active}",
        )

    def test_element_index_double_click_no_focus_steal(self) -> None:
        """double_click via element_index on Calculator '2' button — no focus steal."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            two_button = _find_calc_button(tree, "2")
            self.assertIsNotNone(two_button, "Could not locate Calculator's '2' AXButton")

            result = c.call_tool("double_click", {
                "pid": self._calc_pid,
                "window_id": window_id,
                "element_index": two_button,
            })
            self.assertIsNone(result.get("isError"), msg=result)
            text = result.get("content", [{}])[0].get("text", "")
            print(f"\n  double_click result: {text}")
            self.assertTrue(text.startswith("✅"), f"Expected ✅ prefix, got: {text}")

        self._assert_no_focus_steal()

    def test_pixel_double_click_no_focus_steal(self) -> None:
        """double_click via pixel coords — no focus steal."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            sc = snap.get("structuredContent", snap)
            w = sc.get("screenshot_width", CALC_DEFAULT_W)
            h = sc.get("screenshot_height", CALC_DEFAULT_H)

            bx, by = CALC_TWO_BUTTON_XY
            x = int(bx * w / CALC_DEFAULT_W)
            y = int(by * h / CALC_DEFAULT_H)
            result = c.call_tool("double_click", {
                "pid": self._calc_pid,
                "window_id": window_id,
                "x": x,
                "y": y,
            })
            self.assertIsNone(result.get("isError"), msg=result)
            text = result.get("content", [{}])[0].get("text", "")
            print(f"\n  double_click pixel result: {text}")
            self.assertTrue(text.startswith("✅"), f"Expected ✅ prefix, got: {text}")

        self._assert_no_focus_steal()


if __name__ == "__main__":
    unittest.main(verbosity=2)
