"""Integration test: launch_app opens a visible window without stealing focus.

Root cause being tested
-----------------------
Previously, `launch_app` set `config.hides = true` during launch to complete
the launch lifecycle, which left the app's window invisible. After removing
that flag, cold-launched apps now have their windows visible on screen.

Additionally, `NSRunningApplication.unhide()` is called after launch so any
app that did hide itself during its own launch lifecycle becomes visible
without requiring a separate `unhide` step from the caller.

Behavioral contract verified by this test
-----------------------------------------
After `launch_app(bundle_id=...)` WITHOUT any `urls` parameter:
  • The app has at least one window with `is_on_screen: true`.
  • FocusMonitorApp retains focus — 0 focus losses (no activation occurred).

Test subjects
-------------
TextEdit and Calculator: simple system apps that:
  - Do NOT auto-relaunch when killed (unlike Finder)
  - Always open with at least one window on cold launch
  - Are installed at known paths on all macOS systems

Run:
    scripts/test.sh test_launch_app_visible
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
)

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))
_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_BUNDLE = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.app")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_BUNDLE, "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"

TEXTEDIT_BUNDLE = "com.apple.TextEdit"
CALCULATOR_BUNDLE = "com.apple.calculator"
FINDER_BUNDLE = "com.apple.finder"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _tool_text(result: dict) -> str:
    for item in result.get("content", []):
        if item.get("type") == "text":
            return item.get("text", "")
    return ""


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
            pid = int(line.split("=", 1)[1])
            return proc, pid
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError("FocusMonitorApp did not print FOCUS_PID in time")


def _read_focus_losses() -> int:
    try:
        with open(_LOSS_FILE) as f:
            return int(f.read().strip())
    except (FileNotFoundError, ValueError):
        return 0


def _activate_focus_monitor(fm_pid: int) -> None:
    subprocess.run(
        ["osascript", "-e",
         f'tell application "System Events" to set frontmost of '
         f'(first process whose unix id is {fm_pid}) to true'],
        check=False,
    )
    time.sleep(0.5)


def _on_screen_windows(client: DriverClient, pid: int) -> list[dict]:
    """Return windows that are on screen with a reasonable size."""
    result = client.call_tool("list_windows", {"pid": pid})
    windows = result.get("structuredContent", {}).get("windows", [])
    return [
        w for w in windows
        if w.get("is_on_screen")
        and (w.get("bounds", {}).get("width", 0) or 0) > 50
        and (w.get("bounds", {}).get("height", 0) or 0) > 50
    ]


def _launch_app_pid(client: DriverClient, bundle_id: str) -> int:
    """Call launch_app and extract the pid from the result text."""
    result = client.call_tool("launch_app", {"bundle_id": bundle_id})
    text = _tool_text(result)
    m = re.search(r'pid[=:\s]+(\d+)', text, re.IGNORECASE)
    if not m:
        raise RuntimeError(
            f"launch_app({bundle_id!r}) did not return pid. Response: {text[:300]}"
        )
    return int(m.group(1))


# ---------------------------------------------------------------------------
# Test class
# ---------------------------------------------------------------------------

class TestLaunchAppVisible(unittest.TestCase):
    """launch_app opens an on-screen window without stealing focus."""

    binary: str
    client: DriverClient
    fm_proc: subprocess.Popen
    fm_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
        time.sleep(0.3)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

        # Kill test apps so each test starts from a clean state.
        subprocess.run(["pkill", "-x", "TextEdit"], check=False)
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        subprocess.run(["pkill", "-x", "Finder"], check=False)
        time.sleep(1.5)  # Finder takes a moment to be fully killed + relaunched by macOS

        cls.binary = default_binary_path()
        cls.client = DriverClient(cls.binary).__enter__()

        # Build + launch FocusMonitorApp BEFORE launch_app so it owns the
        # foreground while we measure focus losses.
        _build_focus_app()
        cls.fm_proc, cls.fm_pid = _launch_focus_app()
        time.sleep(0.8)

    @classmethod
    def tearDownClass(cls) -> None:
        cls.client.__exit__(None, None, None)
        cls.fm_proc.terminate()
        try:
            cls.fm_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            cls.fm_proc.kill()
        subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
        subprocess.run(["pkill", "-x", "TextEdit"], check=False)
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        # Leave Finder running — macOS auto-relaunches it anyway.

    def setUp(self) -> None:
        _activate_focus_monitor(self.fm_pid)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    # -----------------------------------------------------------------------
    # Test 01: TextEdit cold launch → visible window, no focus steal
    # -----------------------------------------------------------------------

    def test_01_textedit_cold_launch_opens_visible_window(self) -> None:
        """launch_app(TextEdit) must open a visible on-screen window.

        Verifies:
          • The returned result includes a non-zero PID.
          • list_windows for that PID shows at least one window with
            is_on_screen == true and a reasonable size.
          • FocusMonitorApp did NOT lose focus (activates=false contract).
        """
        _activate_focus_monitor(self.fm_pid)
        losses_before = _read_focus_losses()

        pid = _launch_app_pid(self.client, TEXTEDIT_BUNDLE)
        self.assertGreater(pid, 0, "TextEdit PID must be positive")

        # Allow time for TextEdit to open its window.
        time.sleep(2.0)

        on_screen = _on_screen_windows(self.client, pid)
        losses = _read_focus_losses() - losses_before

        self.assertGreater(
            len(on_screen), 0,
            f"launch_app(TextEdit) must produce at least one on-screen window. "
            f"pid={pid}. All windows: "
            f"{self.client.call_tool('list_windows', {'pid': pid}).get('structuredContent', {}).get('windows', [])}"
        )
        self.assertEqual(
            losses, 0,
            f"launch_app must NOT steal focus. Got {losses} focus loss(es). "
            f"The activates=false contract was violated."
        )
        print(
            f"\n  ✓ TextEdit pid={pid} has {len(on_screen)} on-screen window(s)"
        )
        print(f"  ✓ Focus losses: {losses} (expected 0)")

    # -----------------------------------------------------------------------
    # Test 02: Calculator cold launch → visible window, no focus steal
    # -----------------------------------------------------------------------

    def test_02_calculator_cold_launch_opens_visible_window(self) -> None:
        """launch_app(Calculator) must open a visible on-screen window.

        Calculator is notable because it calls NSApp.activate in its own
        applicationDidFinishLaunching, testing that launch_app's focus-steal
        suppressor correctly returns focus to FocusMonitorApp even when the
        target app self-activates.
        """
        # Ensure Calculator is dead before this test.
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.5)

        _activate_focus_monitor(self.fm_pid)
        losses_before = _read_focus_losses()

        pid = _launch_app_pid(self.client, CALCULATOR_BUNDLE)
        self.assertGreater(pid, 0, "Calculator PID must be positive")

        # Calculator is fast to launch but may self-activate; give the
        # focus-steal suppressor time to restore FocusMonitorApp.
        time.sleep(2.5)

        on_screen = _on_screen_windows(self.client, pid)

        # Check focus was restored — FocusMonitorApp must be frontmost now
        # (even if Calculator self-activated briefly and got suppressed).
        import subprocess as _sp
        result = _sp.run(
            ["osascript", "-e",
             "tell application \"System Events\" to get unix id of first process "
             "whose frontmost is true"],
            capture_output=True, text=True
        )
        frontmost_pid = int(result.stdout.strip()) if result.stdout.strip().isdigit() else -1

        self.assertGreater(
            len(on_screen), 0,
            f"launch_app(Calculator) must produce at least one on-screen window. "
            f"pid={pid}. All windows: "
            f"{self.client.call_tool('list_windows', {'pid': pid}).get('structuredContent', {}).get('windows', [])}"
        )
        self.assertEqual(
            frontmost_pid, self.fm_pid,
            f"After launch_app(Calculator), FocusMonitorApp (pid={self.fm_pid}) "
            f"must be frontmost, but pid={frontmost_pid} is frontmost. "
            f"focus-steal suppressor failed."
        )
        print(
            f"\n  ✓ Calculator pid={pid} has {len(on_screen)} on-screen window(s)"
        )
        print(
            f"  ✓ FocusMonitorApp (pid={self.fm_pid}) is still frontmost — "
            f"focus-steal suppressed"
        )

    # -----------------------------------------------------------------------
    # Test 03: Finder — already running, no on-screen windows
    # -----------------------------------------------------------------------

    def test_03_finder_no_url_opens_visible_window(self) -> None:
        """launch_app(Finder) without urls must open a visible Finder window.

        Finder is always running (macOS auto-relaunches it after pkill). When
        called without urls, launch_app must automatically open a home-directory
        window on the current Space using the application(_:open:) URL-handoff
        path — no explicit url parameter required from the caller.

        Verifies:
          • At least one Finder window with is_on_screen == true.
          • FocusMonitorApp did NOT lose focus (0 losses — no activation needed).
        """
        _activate_focus_monitor(self.fm_pid)
        losses_before = _read_focus_losses()

        pid = _launch_app_pid(self.client, FINDER_BUNDLE)
        self.assertGreater(pid, 0, "Finder PID must be positive")

        # Give Finder time to create its window via application(_:open:).
        time.sleep(2.0)

        on_screen = _on_screen_windows(self.client, pid)
        losses = _read_focus_losses() - losses_before

        self.assertGreater(
            len(on_screen), 0,
            f"launch_app(Finder) without urls must produce at least one on-screen "
            f"window (home-directory fallback). pid={pid}. "
            f"All windows: {self.client.call_tool('list_windows', {'pid': pid}).get('structuredContent', {}).get('windows', [])}"
        )
        self.assertEqual(
            losses, 0,
            f"launch_app(Finder) must NOT steal focus. Got {losses} focus loss(es). "
            f"The home-directory fallback must use activates=false."
        )
        print(f"\n  ✓ Finder pid={pid} has {len(on_screen)} on-screen window(s)")
        print(f"  ✓ Focus losses: {losses} (expected 0 — no activation)")


if __name__ == "__main__":
    unittest.main()
