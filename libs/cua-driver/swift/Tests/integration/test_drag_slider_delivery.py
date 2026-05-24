"""Integration test: pixel drag on backgrounded Safari range slider.

Drives `<input type="range" id="f-range" min="0" max="100" value="50">`
in `fixtures/form_all_inputs.html`, opened in Safari with `open -g` so
Safari is NOT frontmost, with FocusMonitorApp launched on top.

Asserts:
  - Drag delivery: the slider's value changed (default 50 → ≥80 after a
    rightward drag past the slider's right edge — WebKit clamps to max=100).
  - No focus steal: FocusMonitorApp stays frontmost throughout.

The drag tool is pixel-only (macOS AX has no semantic drag action), so
this test exercises the path that would otherwise be silently broken by
a wrong coord-space conversion or a focus-stealing recipe.

Run:
    scripts/test.sh test_drag_slider_delivery
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import (  # noqa: E402
    DriverClient,
    default_binary_path,
    frontmost_bundle_id,
    resolve_window_id,
)

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))

_HTML_FORM = os.path.join(_THIS_DIR, "fixtures", "form_all_inputs.html")
_FORM_URL = f"file://{_HTML_FORM}"

_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_BUNDLE = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.app")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_BUNDLE, "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"

SAFARI_BUNDLE = "com.apple.Safari"
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


def _safari_js(expr: str, timeout: float = 5.0) -> str:
    """Evaluate a JS expression in Safari's front document via osascript."""
    script = (
        f'tell application "Safari" to do JavaScript "{expr}" in front document'
    )
    r = subprocess.run(
        ["osascript", "-e", script],
        capture_output=True, text=True, timeout=timeout,
    )
    return r.stdout.strip()


def _wait_for_form(timeout: float = 25.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _safari_js("typeof getFieldValues") == "function":
            return True
        time.sleep(0.5)
    return False


def _slider_value() -> int:
    """Read the current slider value via the fixture's getFieldValues()."""
    raw = _safari_js("getFieldValues()")
    try:
        v = json.loads(raw).get("range", "")
    except (json.JSONDecodeError, AttributeError):
        return -1
    try:
        return int(v)
    except (TypeError, ValueError):
        return -1


def _slider_screen_rect() -> dict:
    """Return the slider's screen-absolute bounding box in screen points.

    Combines viewport-local `getBoundingClientRect()` with
    `window.screenLeft / screenTop` (origin of the viewport in screen
    coordinates). Both are CSS pixels, which equal macOS screen points
    at the default zoom — so the result is directly usable as a target
    for `WindowCoordinateSpace.screenPoint(...)` after subtracting the
    window's `bounds.x/y` to get window-local points, then multiplying
    by the screenshot scale factor to get image pixels.
    """
    expr = (
        "JSON.stringify((function(){"
        "var r=document.getElementById('f-range').getBoundingClientRect();"
        "return {left:r.left,top:r.top,width:r.width,height:r.height,"
        "screenLeft:window.screenLeft,screenTop:window.screenTop};"
        "})())"
    )
    raw = _safari_js(expr)
    # AppleScript wraps the JSON in extra quotes/escapes — strip them.
    raw = raw.replace('\\"', '"').strip('"')
    return json.loads(raw)


class SafariRangeSliderDragDelivery(unittest.TestCase):
    """Drag the range slider in backgrounded Safari — value changes, no focus steal."""

    _safari_pid: int
    _focus_proc: subprocess.Popen
    _focus_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        _build_focus_app()
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

        subprocess.run(["pkill", "-x", "Safari"], check=False)
        time.sleep(1.0)
        # `open -g` keeps Safari in the background.
        subprocess.run(["open", "-g", "-a", "Safari", _FORM_URL], check=True)
        if not _wait_for_form():
            raise RuntimeError(
                "Safari did not load the form fixture in time. "
                "Verify Safari → Develop → Allow JavaScript from Apple Events is on."
            )

        cls.binary = default_binary_path()
        with DriverClient(cls.binary) as c:
            apps = c.call_tool("list_apps")["structuredContent"]["apps"]
            safari = [a for a in apps if a.get("bundle_id") == SAFARI_BUNDLE]
            if not safari:
                raise RuntimeError("Safari is not running after open -g")
            cls._safari_pid = safari[0]["pid"]

        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        time.sleep(0.5)

        with DriverClient(cls.binary) as c:
            active = frontmost_bundle_id(c)
            assert active == FOCUS_MONITOR_BUNDLE, (
                f"Expected FocusMonitorApp frontmost at start, got {active}"
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
        subprocess.run(["pkill", "-x", "Safari"], check=False)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    def setUp(self) -> None:
        # Reset the slider to its default mid-track value so the test
        # is independent of any prior run that left it elsewhere.
        _safari_js(
            "(function(){var s=document.getElementById('f-range');"
            "s.value=50;s.dispatchEvent(new Event('input'));"
            "s.dispatchEvent(new Event('change'));})()"
        )
        time.sleep(0.2)
        self._losses_before = _read_focus_losses()

    def test_slider_drag_to_right_edge(self) -> None:
        """Drag the thumb from mid-track to past the right edge — value goes to 100."""
        before = _slider_value()
        self.assertEqual(
            before, 50,
            f"slider didn't reset to 50 (got {before}) — fixture or JS bridge off"
        )

        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._safari_pid)
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._safari_pid, "window_id": window_id},
            )
            sc = snap.get("structuredContent", snap)
            scale = sc.get("screenshot_scale_factor", 2)

            # Window origin in screen points (top-left).
            windows = c.call_tool(
                "list_windows", {"pid": self._safari_pid}
            )["structuredContent"]["windows"]
            win = next(
                w for w in windows
                if w["window_id"] == window_id
            )
            win_x = win["bounds"]["x"]
            win_y = win["bounds"]["y"]

            rect = _slider_screen_rect()
            # Slider center in screen points.
            cx = rect["screenLeft"] + rect["left"] + rect["width"] / 2
            cy = rect["screenTop"]  + rect["top"]  + rect["height"] / 2
            # End point: 50 px past the right edge of the slider, same y.
            ex = rect["screenLeft"] + rect["left"] + rect["width"] + 50
            ey = cy
            # Convert screen points → window-local image pixels.
            from_x = (cx - win_x) * scale
            from_y = (cy - win_y) * scale
            to_x   = (ex - win_x) * scale
            to_y   = (ey - win_y) * scale

            print(
                f"\n  slider rect (screen pts): "
                f"x={cx:.0f}..{ex:.0f}, y={cy:.0f}; "
                f"window-local pixels: ({from_x:.0f},{from_y:.0f}) → ({to_x:.0f},{to_y:.0f})"
            )

            result = c.call_tool(
                "drag",
                {
                    "pid": self._safari_pid,
                    "window_id": window_id,
                    "from_x": from_x,
                    "from_y": from_y,
                    "to_x": to_x,
                    "to_y": to_y,
                    "duration_ms": 500,
                    "steps": 24,
                },
            )
            print(
                f"  drag result: {result.get('content', [{}])[0].get('text', '')[:200]}"
            )

        time.sleep(0.4)
        after = _slider_value()
        print(f"  slider value: {before} → {after}")

        # Drag landed AND clamped at the slider's max (100).
        self.assertGreaterEqual(
            after, 80,
            f"Drag did not move the slider far enough — value is {after} (was {before})"
        )

        # Focus invariant: FocusMonitorApp stayed frontmost throughout.
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        losses = _read_focus_losses()
        print(f"  losses: {self._losses_before}→{losses}, frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen — frontmost is {active} (expected FocusMonitorApp)"
        )
        self.assertEqual(
            losses, self._losses_before,
            f"Focus losses increased: {self._losses_before} → {losses}"
        )


if __name__ == "__main__":
    unittest.main()
