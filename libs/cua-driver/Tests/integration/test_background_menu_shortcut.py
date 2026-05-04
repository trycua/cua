"""Integration test: hotkey with menu key equivalents on a backgrounded app.

Root cause being tested
-----------------------
`hotkey` routes keys via `SLEventPostToPid` which, when given an
`SLSEventAuthenticationMessage` envelope (the default keyboard path),
forks onto a direct-mach delivery path that bypasses `IOHIDPostEvent`.
NSMenu key equivalents are dispatched in `NSApplication.sendEvent:`
which processes the `IOHIDPostEvent` stream — so auth-enveloped events
are silently swallowed by native AppKit apps.

Additionally, NSMenu only dispatches key equivalents to the AppKit-active
process. When an app is backgrounded, `CGEvent.postToPid` reaches its
event queue but NSMenu's filter rejects the shortcut.

Fix: when `window_id` is supplied to `hotkey` / `press_key`:
  1. Call `FocusWithoutRaise.activateForMenuShortcut` — makes target
     WindowServer-frontmost via `SLPSSetFrontProcessWithOptions(kCPSNoWindows)`
     without raising its window or triggering Space follow.
  2. Post via `SLEventPostToPid` WITHOUT the auth-message envelope
     (`attachAuthMessage: false`) — routes through IOHIDPostEvent so
     NSMenu key equivalents fire normally.

The Chromium/renderer path (no `window_id`) is unchanged: auth-message
envelope stays on, since Chromium requires it for its keyboard pipeline.

Test subject: TextEdit
The test uses TextEdit as the backgrounded target app. Two key equivalents:
  - Cmd+Z (Undo): harmless idempotent action used for the negative case.
  - Cmd+N (New Document): opens a new window, easy to count.

Behavioral contract verified by this test
-----------------------------------------
WITHOUT `window_id` (auth-envelope path, no activation):
  • FocusMonitorApp does NOT lose focus — no activation happened.
  • The keystroke goes directly to the target PID via SkyLight direct-mach.

WITH `window_id` (no-auth-envelope path, full activation):
  • FocusMonitorApp loses focus EXACTLY ONCE — activateForMenuShortcut
    called SLPSSetFrontProcessWithOptions(kCPSNoWindows).
  • The NSMenu key equivalent fires: a new TextEdit document opens.

Test plan
---------
1. Kill any existing TextEdit; launch it fresh (one empty document opens).
2. Launch FocusMonitorApp last so it is the foreground sentinel.

Negative case (no side-effects without window_id):
3. `hotkey(pid, ["cmd","z"])` WITHOUT window_id → FocusMonitorApp loses focus
   0 times (no activation triggered; auth path delivers directly to pid).

Positive case (activation + NSMenu fires with window_id):
4. `hotkey(pid, ["cmd","n"], window_id=wid)` WITH window_id → new TextEdit
   document opens (window count +1) AND FocusMonitorApp loses focus exactly
   once (the activate trade-off — same as the click path).
5. Cleanup: close the extra document.

Run:
    scripts/test.sh test_background_menu_shortcut
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


def _textedit_window_count(client: DriverClient, textedit_pid: int) -> int:
    """Count on-screen TextEdit document windows."""
    result = client.call_tool("list_windows", {"pid": textedit_pid})
    windows = result.get("structuredContent", {}).get("windows", [])
    return sum(
        1 for w in windows
        if w.get("is_on_screen")
        and (w.get("bounds", {}).get("width", 0) or 0) > 100
        and (w.get("bounds", {}).get("height", 0) or 0) > 100
    )


def _close_extra_textedit_windows(
    client: DriverClient, textedit_pid: int, target_count: int
) -> None:
    """Close extra TextEdit windows (no-save) until count == target_count."""
    result = client.call_tool("list_windows", {"pid": textedit_pid})
    windows = result.get("structuredContent", {}).get("windows", [])
    on_screen = [
        w for w in windows
        if w.get("is_on_screen")
        and (w.get("bounds", {}).get("width", 0) or 0) > 100
        and (w.get("bounds", {}).get("height", 0) or 0) > 100
    ]
    to_close = on_screen[target_count:]
    for w in to_close:
        wid = w["window_id"]
        # Cmd+W to close; handle any "Don't Save" sheet.
        client.call_tool("hotkey", {
            "pid": textedit_pid,
            "keys": ["cmd", "w"],
            "window_id": wid,
        })
        time.sleep(0.4)
        # Delete = keyboard shortcut for "Don't Save" in macOS sheets.
        client.call_tool("press_key", {
            "pid": textedit_pid,
            "key": "delete",
            "window_id": wid,
        })
        time.sleep(0.3)


# ---------------------------------------------------------------------------
# Test class
# ---------------------------------------------------------------------------

class TestBackgroundMenuShortcut(unittest.TestCase):
    """hotkey with window_id fires NSMenu key equivalents on backgrounded apps."""

    binary: str
    client: DriverClient
    fm_proc: subprocess.Popen
    fm_pid: int
    textedit_pid: int
    textedit_wid: int
    initial_window_count: int

    @classmethod
    def setUpClass(cls) -> None:
        subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
        time.sleep(0.3)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

        # Kill any existing TextEdit to start from a clean state.
        subprocess.run(["pkill", "-x", "TextEdit"], check=False)
        time.sleep(0.5)

        cls.binary = default_binary_path()
        cls.client = DriverClient(cls.binary).__enter__()

        # Launch TextEdit — it starts with one empty document.
        result = cls.client.call_tool("launch_app", {"bundle_id": TEXTEDIT_BUNDLE})
        text = _tool_text(result)
        m = re.search(r'pid[=:\s]+(\d+)', text, re.IGNORECASE)
        if not m:
            raise RuntimeError(f"launch_app TextEdit did not return pid: {text[:200]}")
        cls.textedit_pid = int(m.group(1))
        time.sleep(1.0)

        # Get the TextEdit document window.
        result = cls.client.call_tool("list_windows", {"pid": cls.textedit_pid})
        windows = result.get("structuredContent", {}).get("windows", [])
        on_screen = [
            w for w in windows
            if w.get("is_on_screen")
            and (w.get("bounds", {}).get("width", 0) or 0) > 100
            and (w.get("bounds", {}).get("height", 0) or 0) > 100
        ]
        if not on_screen:
            raise RuntimeError(
                f"No on-screen TextEdit window found. Windows: {windows[:5]}"
            )
        best = max(on_screen, key=lambda w: (
            (w.get("bounds") or {}).get("width", 0) *
            (w.get("bounds") or {}).get("height", 0)
        ))
        cls.textedit_wid = best["window_id"]
        cls.initial_window_count = len(on_screen)

        # Build + launch FocusMonitorApp LAST so it owns the foreground.
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

    def setUp(self) -> None:
        _activate_focus_monitor(self.fm_pid)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    # -----------------------------------------------------------------------
    # Negative case: WITHOUT window_id → no activation (0 focus losses)
    # -----------------------------------------------------------------------

    def test_01_hotkey_without_window_id_does_not_activate(self) -> None:
        """hotkey without window_id must NOT activate the target app.

        Sends Cmd+Z (Undo) to TextEdit via the auth-envelope path (no window_id).
        This delivers the keystroke directly to the target PID without calling
        activateForMenuShortcut, so FocusMonitorApp should retain focus (0 losses).

        The auth-envelope path is the correct route for Chromium targets; for
        native AppKit apps that need NSMenu dispatch, window_id is required.
        """
        _activate_focus_monitor(self.fm_pid)
        losses_before = _read_focus_losses()

        # Cmd+Z (Undo) — harmless idempotent action; won't open new windows.
        self.client.call_tool("hotkey", {
            "pid": self.textedit_pid,
            "keys": ["cmd", "z"],
        })
        time.sleep(1.0)

        losses_after = _read_focus_losses()
        losses = losses_after - losses_before

        self.assertEqual(
            losses, 0,
            f"hotkey WITHOUT window_id should not steal focus from FocusMonitorApp "
            f"(no activation should occur). Got {losses} focus loss(es). "
            f"This means activateForMenuShortcut is being called when it shouldn't be."
        )
        print(f"\n  ✓ Cmd+Z without window_id: 0 focus losses (no activation, as expected)")

    # -----------------------------------------------------------------------
    # Positive case: WITH window_id → activation + NSMenu fires
    # -----------------------------------------------------------------------

    def test_02_hotkey_with_window_id_activates_and_opens_document(self) -> None:
        """hotkey with window_id activates the app and fires NSMenu key equivalents.

        Sends Cmd+N (New Document) to a backgrounded TextEdit window.
        Fix: activateForMenuShortcut calls SLPSSetFrontProcessWithOptions
        (kCPSNoWindows) to make TextEdit WindowServer-frontmost; then posts
        without the SkyLight auth-message envelope so the event routes through
        IOHIDPostEvent and NSApplication.sendEvent: dispatches NSMenu key
        equivalents (Cmd+N → New Document).
        """
        count_before = _textedit_window_count(self.client, self.textedit_pid)
        _activate_focus_monitor(self.fm_pid)
        losses_before = _read_focus_losses()

        self.client.call_tool("hotkey", {
            "pid": self.textedit_pid,
            "keys": ["cmd", "n"],
            "window_id": self.textedit_wid,
        })
        time.sleep(1.5)

        count_after = _textedit_window_count(self.client, self.textedit_pid)

        # Clean up extra windows before asserting.
        if count_after > count_before:
            _close_extra_textedit_windows(
                self.client, self.textedit_pid, count_before
            )
            time.sleep(0.5)

        # FocusMonitorApp should lose focus exactly once (the activate trade-off).
        losses_after = _read_focus_losses()
        losses = losses_after - losses_before

        self.assertGreater(
            count_after, count_before,
            f"hotkey WITH window_id should open a new TextEdit document. "
            f"textedit_pid={self.textedit_pid} textedit_wid={self.textedit_wid}. "
            f"Windows before={count_before}, after={count_after}."
        )
        self.assertEqual(
            losses, 1,
            f"Expected exactly 1 focus loss (the activateForMenuShortcut activate), "
            f"got {losses}."
        )

        print(f"\n  ✓ Cmd+N with window_id opened a new TextEdit document "
              f"(count {count_before} → {count_after})")
        print(f"  ✓ Focus losses: {losses} (expected 1)")


if __name__ == "__main__":
    unittest.main()
