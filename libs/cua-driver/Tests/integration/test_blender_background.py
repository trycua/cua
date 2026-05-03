"""Integration test: click Blender in the background without stealing focus.

Blender uses a custom GHOST/OpenGL viewport — AX-addressed clicks
don't reach it (the AX tree only exposes window chrome). Pixel-
addressed clicks via the HID tap path work when Blender is frontmost,
but the pid-routed paths (SkyLight/CGEvent) are filtered by GHOST.

This test verifies:
  1. get_window_state on backgrounded Blender doesn't steal focus.
  2. Pixel click on backgrounded Blender doesn't steal focus.
  3. press_key on backgrounded Blender doesn't steal focus.

FocusMonitorApp sits on top as the focus-steal witness.

Prerequisites:
  - Blender installed at /Applications/Blender.app

Run:
    CUA_DRIVER_BINARY=... python3 -m unittest Tests.integration.test_blender_background
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

BLENDER_BUNDLE = "org.blenderfoundation.blender"
FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"


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


def _activate_focus_monitor() -> None:
    subprocess.run(
        ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
        check=False,
    )
    time.sleep(0.5)


@unittest.skipUnless(
    os.path.exists("/Applications/Blender.app"),
    "Blender not installed"
)
class BlenderBackgroundTests(unittest.TestCase):
    """Interact with backgrounded Blender without stealing focus."""

    _blender_pid: int
    _focus_proc: subprocess.Popen
    _focus_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        _build_focus_app()
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

        cls.binary = default_binary_path()

        # Find or launch Blender.
        with DriverClient(cls.binary) as c:
            apps = c.call_tool("list_apps")["structuredContent"]["apps"]
            blender = [a for a in apps if a.get("bundle_id") == BLENDER_BUNDLE]
            if blender:
                cls._blender_pid = blender[0]["pid"]
            else:
                result = c.call_tool("launch_app", {"bundle_id": BLENDER_BUNDLE})
                cls._blender_pid = result["structuredContent"]["pid"]
                time.sleep(3.0)  # Blender takes a moment to start
        print(f"\n  Blender pid: {cls._blender_pid}")

        # Launch FocusMonitorApp on top.
        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        print(f"  FocusMonitor pid: {cls._focus_pid}")
        time.sleep(1.0)

        with DriverClient(cls.binary) as c:
            active = frontmost_bundle_id(c)
            assert active == FOCUS_MONITOR_BUNDLE, (
                f"Expected FocusMonitorApp frontmost, got {active}"
            )

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        try:
            cls._focus_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            cls._focus_proc.kill()
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    def setUp(self) -> None:
        _activate_focus_monitor()
        self._losses_before = _read_focus_losses()

    def _assert_focus_preserved(self, label: str) -> None:
        time.sleep(0.3)
        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        loss_delta = losses - self._losses_before
        print(f"  [{label}] losses: {self._losses_before}->{losses} "
              f"(delta={loss_delta}), frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"[{label}] Focus stolen — frontmost is {active}",
        )

    # -- get_window_state should be safe (read-only snapshot) -------------

    def test_01_get_window_state_safe(self) -> None:
        """AX snapshot + screenshot of backgrounded Blender — no focus steal."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._blender_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._blender_pid, "window_id": window_id},
            )
            sc = snap.get("structuredContent", snap)
            has_shot = sc.get("has_screenshot", False)
            count = sc.get("element_count", 0)
            print(f"\n  has_screenshot: {has_shot}, elements: {count}")

        self._assert_focus_preserved("01_get_window_state")

    # -- Pixel click into the 3D viewport ----------------------------------

    def test_02_pixel_click_viewport(self) -> None:
        """Pixel-click the center of Blender's viewport — no focus steal."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._blender_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._blender_pid, "window_id": window_id},
            )
            sc = snap.get("structuredContent", snap)
            w = sc.get("screenshot_width", 0)
            h = sc.get("screenshot_height", 0)
            if not w or not h:
                self.skipTest("No Blender screenshot (window not on-screen)")

            # Click center of the viewport
            x, y = w // 2, h // 2
            print(f"\n  pixel click at ({x}, {y}) in {w}x{h}")
            result = c.call_tool("click", {
                "pid": self._blender_pid,
                "window_id": window_id,
                "x": x,
                "y": y,
            })
            sr = result.get("structuredContent", result)
            print(f"  result: screen ({sr.get('screen_x')}, {sr.get('screen_y')})")

        time.sleep(0.5)
        self._assert_focus_preserved("02_pixel_click_viewport")

    # -- press_key to Blender (keyboard shortcut) --------------------------

    def test_03_press_key_safe(self) -> None:
        """Send a key to backgrounded Blender — no focus steal."""
        with DriverClient(self.binary) as c:
            # Press 'a' — in Blender this toggles select-all
            result = c.call_tool("press_key", {
                "pid": self._blender_pid, "key": "a",
            })
            print(f"\n  press_key result: {result}")

        time.sleep(0.3)
        self._assert_focus_preserved("03_press_key")

    # -- hotkey to Blender (Shift+A = Add menu) ----------------------------

    def test_04_hotkey_safe(self) -> None:
        """Send Shift+A (Add menu) to backgrounded Blender — no focus steal."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("hotkey", {
                "pid": self._blender_pid, "keys": ["shift", "a"],
            })
            print(f"\n  hotkey result: {result}")

        time.sleep(0.3)
        self._assert_focus_preserved("04_hotkey")

    # -- Multiple pixel clicks (simulating viewport interaction) -----------

    def test_05_multiple_pixel_clicks(self) -> None:
        """Multiple pixel clicks in Blender's viewport — no focus steal."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._blender_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._blender_pid, "window_id": window_id},
            )
            sc = snap.get("structuredContent", snap)
            w = sc.get("screenshot_width", 0)
            h = sc.get("screenshot_height", 0)
            if not w or not h:
                self.skipTest("No Blender screenshot")

            # Click a few points in the viewport
            points = [
                (w // 3, h // 3),
                (w // 2, h // 2),
                (2 * w // 3, 2 * h // 3),
            ]
            for x, y in points:
                c.call_tool("click", {
                    "pid": self._blender_pid,
                    "window_id": window_id,
                    "x": x,
                    "y": y,
                })
                time.sleep(0.15)
            print(f"\n  clicked {len(points)} points")

        time.sleep(0.3)
        self._assert_focus_preserved("05_multiple_clicks")


if __name__ == "__main__":
    unittest.main(verbosity=2)
