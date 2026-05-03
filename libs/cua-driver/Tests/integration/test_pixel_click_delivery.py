"""Integration test: pixel-click delivery to backgrounded apps (Modality B).

Reproduces the bug where simulate_click (Modality B) events deliver
but AppKit hit-test drops them.

Sends pixel-addressed clicks (x, y — NOT element_index) to Calculator
while it's backgrounded behind FocusMonitorApp, then asserts:
  - Calculator display changed (click delivery).
  - FocusMonitorApp is still frontmost (no focus steal).

Run:
    scripts/test.sh test_pixel_click_delivery
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
    # Run the binary directly so we can read FOCUS_PID from its stdout, but
    # follow up with `osascript ... to activate` because direct subprocess
    # launches don't go through LaunchServices and so NSApp.activate() in
    # the app itself is rejected as a non-user-initiated activation.
    proc = subprocess.Popen(
        [_FOCUS_APP_EXE],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    pid: int | None = None
    for _ in range(40):
        line = proc.stdout.readline().strip()
        if line.startswith("FOCUS_PID="):
            pid = int(line.split("=", 1)[1])
            break
        time.sleep(0.1)
    if pid is None:
        proc.terminate()
        raise RuntimeError("FocusMonitorApp did not print FOCUS_PID in time")
    # Explicitly activate — this is the routine that "open -a" performs and
    # subprocess.Popen does not.
    subprocess.run(
        ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
        check=False, timeout=3,
    )
    return proc, pid


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
    """Pixel-click digits on backgrounded Calculator — assert delivery + no focus steal."""

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
        time.sleep(0.3)

        cls.binary = default_binary_path()

        # Launch Calculator
        with DriverClient(cls.binary) as c:
            result = c.call_tool("launch_app", {"bundle_id": CALC_BUNDLE})
            cls._calc_pid = result["structuredContent"]["pid"]
        print(f"\n  Calculator pid: {cls._calc_pid}")
        time.sleep(1.5)

        # Launch FocusMonitorApp on top
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
        # Clear via AX before the test
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._calc_pid, "window_id": window_id},
            )
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn = _find_calc_button(tree, "All Clear") or _find_calc_button(tree, "Clear")
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
        # Re-activate FocusMonitorApp after AX clear
        subprocess.run(
            ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
            check=False,
        )
        time.sleep(0.5)
        self._losses_before = _read_focus_losses()

    def test_pixel_click_123(self) -> None:
        """Pixel-click 1, 2, 3 on backgrounded Calculator — display shows '123', no focus steal."""
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
                result = c.call_tool(
                    "click",
                    {
                        "pid": self._calc_pid,
                        "window_id": window_id,
                        "x": x,
                        "y": y,
                    },
                )
                sr = result.get("structuredContent", result)
                print(f"    {digit}: pixel ({x},{y}) -> screen ({sr.get('screen_x')},{sr.get('screen_y')})")
                time.sleep(0.15)

        time.sleep(0.3)

        # Assert delivery
        with DriverClient(self.binary) as c:
            display = _calc_display(c, self._calc_pid)
        print(f"  display: '{display}'")
        self.assertEqual(display, "123", f"Pixel clicks did not land — display is '{display}'")

        # Assert no focus steal
        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        print(f"  losses: {self._losses_before}->{losses}, frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen — frontmost is {active}",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
