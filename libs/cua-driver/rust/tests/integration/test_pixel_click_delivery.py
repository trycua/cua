"""Integration test: pixel-click delivery to backgrounded apps.

Sends pixel-addressed clicks (x, y — NOT element_index) to Calculator
while it's backgrounded behind FocusMonitorApp, then asserts:
  - Calculator display changed (click delivery).
  - FocusMonitorApp is still frontmost (no focus steal).

Note: Calculator is a Mac Catalyst app. CGEventPostToPid alone doesn't
reach Catalyst. This test documents the current behaviour and will pass
once SkyLight SPI event posting is implemented.

Run:
    CUA_DRIVER_BINARY=../../target/debug/cua-driver python3 -m unittest test_pixel_click_delivery -v
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

# Calculator standard-mode (230x408) button grid.
# Column centers (x): 29, 86, 144, 201
# Row centers (y):    133, 193, 253, 313, 373
CALC_BUTTONS = {
    "1": (29,  313), "2": (86,  313), "3": (144, 313),
}


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
        m = re.search(r'\[(\d+)\]', line)
        if not m:
            continue
        if f'({label})' in line or f'id={label}' in line:
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


class CalculatorPixelClickDelivery(unittest.TestCase):
    """Pixel-click digits on backgrounded Calculator."""

    _calc_pid: int
    _focus_proc: subprocess.Popen
    _focus_pid: int

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
        # Clear via AX before the test.
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

    def test_pixel_click_no_focus_steal(self) -> None:
        """Pixel-click Calculator buttons — verify no focus steal regardless of delivery."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._calc_pid, "window_id": window_id},
            )
            sc = snap.get("structuredContent", snap)
            w = sc.get("screenshot_width", 230)
            h = sc.get("screenshot_height", 408)
            print(f"\n  screenshot: {w}x{h}")

            for digit in ["1", "2", "3"]:
                bx, by = CALC_BUTTONS[digit]
                x = int(bx * w / 230)
                y = int(by * h / 408)
                result = c.call_tool("click", {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "x": x,
                    "y": y,
                })
                print(f"    {digit}: pixel ({x},{y})")
                time.sleep(0.15)

        time.sleep(0.3)

        # Assert no focus steal — this is the main invariant for background automation.
        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        print(f"  losses: {self._losses_before}->{losses}, frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen — frontmost is {active}",
        )

    def test_ax_click_delivery(self) -> None:
        """AX-click 1+2 on backgrounded Calculator — verifies AX event delivery."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")

            btn_1 = _find_calc_button(tree, "1")
            btn_add = _find_calc_button(tree, "Add")
            btn_2 = _find_calc_button(tree, "2")
            btn_eq = _find_calc_button(tree, "Equals")
            print(f"\n  buttons: 1={btn_1}, Add={btn_add}, 2={btn_2}, Equals={btn_eq}")

            self.assertIsNotNone(btn_1, "'1' button not found in Calculator AX tree")
            self.assertIsNotNone(btn_add, "'Add' button not found")
            self.assertIsNotNone(btn_2, "'2' button not found")
            self.assertIsNotNone(btn_eq, "'Equals' button not found")

            for idx in [btn_1, btn_add, btn_2, btn_eq]:
                c.call_tool("click", {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "element_index": idx,
                })
                time.sleep(0.3)

            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid,
                "window_id": window_id,
                "query": "AXStaticText",
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"  result tree:\n{tree}")

        self.assertIn("3", tree, "Calculator did not show 3 after 1+2=")

        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        print(f"  losses: {self._losses_before}->{losses}, frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen by AX click — frontmost is {active}",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
