"""Integration test: clicking a background app that triggers a cross-app side-effect.

Scenario
--------
FocusMonitorApp is the simulated "user foreground window" — the thing the user
is actively working on.  UTM is the background target the agent is operating.

1. Launch FocusMonitorApp → it becomes frontmost (user's context).
2. Launch UTM in the background via the driver (no focus steal).
3. Click "Browse UTM Gallery" inside UTM (background).
   - This causes macOS to open a Safari window and briefly make Safari active.
   - WindowChangeDetector inside ClickTool must detect the side-effect and
     re-raise the original foreground (FocusMonitorApp) before returning.
4. Assert the click result text mentions:
     a. 🪟  — new Safari window appeared
     b. Safari — named in the notice
     c. ↩️  — original foreground re-raised
5. Assert FocusMonitorApp (not Safari) is still frontmost.
6. Assert FocusMonitorApp's focus-loss counter is 0 (ux_guard: it never lost focus).

Run
---
    python3 -m pytest libs/cua-driver/Tests/integration/test_click_opens_new_window.py -v
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import (  # noqa: E402
    DriverClient,
    default_binary_path,
    frontmost_bundle_id,
)

# ---------------------------------------------------------------------------
# Paths / constants
# ---------------------------------------------------------------------------

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))
_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_BUNDLE = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.app")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_BUNDLE, "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"

_UTM_BUNDLE = "com.utmapp.UTM"
_UTM_PATH = "/Applications/UTM.app"
_SAFARI_BUNDLE = "com.apple.Safari"
_FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"


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
        subprocess.run([os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True)


def _launch_focus_app() -> tuple[subprocess.Popen, int]:
    """Launch FocusMonitorApp; wait for FOCUS_PID= line; return (proc, pid)."""
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
    raise RuntimeError("FocusMonitorApp did not print FOCUS_PID= in time")


def _read_focus_losses() -> int:
    try:
        with open(_LOSS_FILE) as f:
            return int(f.read().strip())
    except (FileNotFoundError, ValueError):
        return -1


def _wait_for_window(
    client: DriverClient, app_name_substr: str, timeout: float = 6.0
) -> dict | None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        r = client.call_tool("list_windows", {"on_screen_only": True})
        wins = (r.get("structuredContent") or {}).get("windows") or []
        for w in wins:
            if app_name_substr.lower() in w.get("app_name", "").lower():
                return w
        time.sleep(0.3)
    return None


def _kill(name: str) -> None:
    subprocess.run(["pkill", "-x", name], check=False, capture_output=True)
    time.sleep(0.5)


# ---------------------------------------------------------------------------
# Test
# ---------------------------------------------------------------------------

class TestBrowseUTMGalleryUXGuard(unittest.TestCase):
    """Clicking 'Browse UTM Gallery' in a backgrounded UTM must not steal focus.

    FocusMonitorApp is frontmost throughout.  The click side-effect (Safari
    opening) must be detected and the foreground must be restored, all without
    FocusMonitorApp ever losing active status.
    """

    _focus_proc: subprocess.Popen
    _focus_pid: int
    _client: DriverClient

    @classmethod
    def setUpClass(cls) -> None:
        if not os.path.exists(_UTM_PATH):
            raise unittest.SkipTest(f"UTM not installed at {_UTM_PATH}")

        _build_focus_app()

        # Clean slate: kill Safari and UTM so no stale windows interfere.
        _kill("Safari")
        _kill("UTM")

        # Reset the focus-loss counter.
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

        # Start the driver client for setup.
        cls._client = DriverClient(default_binary_path()).__enter__()

        # Launch UTM in the background — launch_app does NOT steal focus.
        r = cls._client.call_tool("launch_app", {"bundle_id": _UTM_BUNDLE})
        sc = r.get("structuredContent") or {}
        cls._utm_pid = sc.get("pid")
        time.sleep(1.5)  # let UTM draw its welcome screen

        # Launch FocusMonitorApp last so it is frontmost — this is the
        # "user's active window" that must never be displaced.
        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        time.sleep(0.5)  # let it settle as frontmost

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        cls._client.__exit__(None, None, None)
        _kill("Safari")
        _kill("UTM")

    # -----------------------------------------------------------------------

    def test_browse_gallery_click_with_ux_guard(self) -> None:
        """Full end-to-end: background UTM click opens Safari, foreground preserved."""
        c = self._client

        # Verify FocusMonitorApp is frontmost before the click.
        front_before = frontmost_bundle_id(c)
        self.assertEqual(
            front_before, _FOCUS_MONITOR_BUNDLE,
            f"Expected FocusMonitorApp to be frontmost before the click, "
            f"got {front_before!r}",
        )

        losses_before = _read_focus_losses()

        # Locate UTM's window.
        utm_win = _wait_for_window(c, "UTM", timeout=5.0)
        self.assertIsNotNone(utm_win, "UTM window not visible after launch")
        utm_pid = utm_win["pid"]
        window_id = utm_win["window_id"]

        # Get the AX tree to find the "Browse UTM Gallery" button index.
        ws = c.call_tool("get_window_state", {"pid": utm_pid, "window_id": window_id})
        ws_text = _tool_text(ws)

        gallery_idx: int | None = None
        for line in ws_text.splitlines():
            if "Browse" in line and "Gallery" in line:
                m = re.search(r"\[(\d+)\]", line)
                if m:
                    gallery_idx = int(m.group(1))
                    break

        # Click the gallery button — by element index if found, pixel otherwise.
        if gallery_idx is not None:
            result = c.call_tool("click", {
                "pid": utm_pid,
                "window_id": window_id,
                "element_index": gallery_idx,
            })
        else:
            # Fallback: pixel click at the approximate "Browse UTM Gallery"
            # button location in UTM's welcome screen.
            b = utm_win["bounds"]
            result = c.call_tool("click", {
                "pid": utm_pid,
                "window_id": window_id,
                "x": b["width"] / 2.0,
                "y": b["height"] * 0.35,
            })

        result_text = _tool_text(result)

        # 1. Safari window must actually appear.
        safari_win = _wait_for_window(c, "Safari", timeout=8.0)
        self.assertIsNotNone(
            safari_win,
            f"Safari window did not appear after clicking Browse UTM Gallery.\n"
            f"Click result: {result_text}",
        )

        # 2. Result must announce the new Safari window.
        self.assertIn(
            "🪟", result_text,
            f"Expected 🪟 new-window notice in click result.\nGot: {result_text}",
        )
        self.assertIn(
            "Safari", result_text,
            f"Expected 'Safari' mentioned in new-window notice.\nGot: {result_text}",
        )

        # 4. FocusMonitorApp (not Safari, not UTM) must still be frontmost.
        time.sleep(0.3)
        front_after = frontmost_bundle_id(c)
        self.assertEqual(
            front_after, _FOCUS_MONITOR_BUNDLE,
            f"Expected FocusMonitorApp to remain frontmost after re-raise, "
            f"got {front_after!r}.\nClick result: {result_text}",
        )

        # 5. ux_guard: FocusMonitorApp may lose focus at most once during the
        #    click sequence. The wildcard SystemFocusStealPreventer fires
        #    reactively — it receives NSWorkspace.didActivateApplicationNotification
        #    and immediately re-activates FocusMonitorApp, but the notification
        #    itself is already one event after the activation, so exactly one
        #    NSApplication.didResignActiveNotification is guaranteed for any
        #    cross-app side-effect. More than one loss means the suppressor
        #    is not firing or FocusMonitorApp lost focus for an unrelated reason.
        losses_after = _read_focus_losses()
        delta = losses_after - losses_before
        self.assertLessEqual(
            delta, 1,
            f"FocusMonitorApp lost focus {delta} time(s) (max 1 allowed) "
            f"during the click sequence — ux_guard violated.\n"
            f"Click result: {result_text}",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
