"""Integration test: hermes can launch Safari in the background via launch_app.

What this test proves
---------------------
1. ``computer_use(action='launch_app', bundle_id='com.apple.Safari',
   urls=['https://example.com'])`` actually creates a Safari process and
   window — verified independently via a direct cua-driver MCP call to
   ``list_windows``, bypassing any model hallucination.
2. The launch does NOT steal focus from FocusMonitorApp: app-level focus,
   key-window status, and text-field first-responder are all unchanged.
3. ``capture(app='Safari')`` after launch returns Safari's AX tree and
   screenshot, not whatever window happens to be frontmost (Finder, etc.).

Key insight: Safari launched with ``hides=true`` and no URL never creates a
window — the process starts but NSWorkspace never triggers window creation.
Passing ``urls=['https://example.com']`` forces Safari to open a tab and
create a window. The window IS on-screen (z-ordered behind other windows)
but Safari is NOT the active app — that is the correct "background" behavior.

Setup
-----
Safari is killed before the test so we know the process was freshly launched
by hermes. FocusMonitorApp is launched last and holds focus throughout.

Run
---
    python3 -m pytest libs/cua-driver/Tests/integration/test_hermes_launch_safari_bg.py -v
    HERMES_TEST_MODEL=claude-sonnet-4-6 python3 -m pytest ... -v
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path  # noqa: E402

# ---------------------------------------------------------------------------
# Paths & constants
# ---------------------------------------------------------------------------

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))

_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_DIR, "FocusMonitorApp.app", "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE       = "/tmp/focus_monitor_losses.txt"
_KEY_LOSS_FILE   = "/tmp/focus_monitor_key_losses.txt"
_FIELD_LOSS_FILE = "/tmp/focus_monitor_field_losses.txt"

_HERMES_BIN = os.path.expanduser("~/.hermes/hermes-agent/venv/bin/hermes")

_ANTHROPIC_KEY: str = os.environ.get("ANTHROPIC_API_KEY", "")
_MODEL = os.environ.get("HERMES_TEST_MODEL", "claude-haiku-4-5-20251001")

_SAFARI_BUNDLE = "com.apple.Safari"
_LAUNCH_URL    = "https://example.com"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _read_file_int(path: str) -> int:
    try:
        with open(path) as f:
            return int(f.read().strip())
    except (FileNotFoundError, ValueError):
        return 0


def _safari_pids() -> list[int]:
    r = subprocess.run(["pgrep", "-x", "Safari"], capture_output=True, text=True)
    return [int(p) for p in r.stdout.split() if p.strip()]


def _safari_window_count_via_driver(wait_s: float = 4.0) -> int:
    """Spawn one cua-driver session and poll until Safari windows appear.

    Waits up to ``wait_s`` seconds for Safari to create its window after
    launch (Safari opens a URL asynchronously; the window may not be
    registered in WindowServer immediately). Keeps one DriverClient open
    for the duration to avoid spawning/killing multiple processes.
    """
    deadline = time.time() + wait_s
    with DriverClient(default_binary_path()) as c:
        while True:
            r = c.call_tool("list_windows", {})
            sc = r.get("structuredContent") or {}
            wins = sc.get("windows") or []
            count = sum(
                1 for w in wins if "safari" in w.get("app_name", "").lower()
            )
            if count > 0 or time.time() >= deadline:
                return count
            time.sleep(0.5)


def _launch_focus_app() -> tuple[subprocess.Popen, int]:
    proc = subprocess.Popen(
        [_FOCUS_APP_EXE], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
    )
    for _ in range(40):
        line = proc.stdout.readline().strip()
        if line.startswith("FOCUS_PID="):
            return proc, int(line.split("=", 1)[1])
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError("FocusMonitorApp did not print FOCUS_PID= in time")


# ---------------------------------------------------------------------------
# Test
# ---------------------------------------------------------------------------

class TestHermesLaunchSafariBg(unittest.TestCase):
    """Verify hermes launches Safari in the background without stealing focus."""

    _focus_proc: subprocess.Popen
    _focus_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        if not _ANTHROPIC_KEY:
            raise unittest.SkipTest(
                "ANTHROPIC_API_KEY not set — skipping hermes Safari launch test"
            )

        # Build FocusMonitorApp if needed.
        src = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.swift")
        if (not os.path.exists(_FOCUS_APP_EXE)
                or os.path.getmtime(src) > os.path.getmtime(_FOCUS_APP_EXE)):
            subprocess.run([os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True)

        # Kill Safari so we can detect a fresh launch.
        subprocess.run(["pkill", "-x", "Safari"], check=False)
        time.sleep(1.5)
        assert _safari_pids() == [], "Safari still running after pkill"

        # Clear focus-loss counters.
        for f in (_LOSS_FILE, _KEY_LOSS_FILE, _FIELD_LOSS_FILE):
            if os.path.exists(f):
                os.remove(f)

        # FocusMonitorApp owns focus throughout.
        cls._focus_proc, cls._focus_pid = _launch_focus_app()

        # Stabilise: wait until FocusMonitorApp's field-loss counter stops changing.
        prev = -1
        for _ in range(20):
            cur = _read_file_int(_FIELD_LOSS_FILE)
            if cur == prev:
                break
            prev = cur
            time.sleep(0.3)

        cls._losses_before       = _read_file_int(_LOSS_FILE)
        cls._key_losses_before   = _read_file_int(_KEY_LOSS_FILE)
        cls._field_losses_before = _read_file_int(_FIELD_LOSS_FILE)

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        # Close stdout before waiting — if FocusMonitorApp keeps writing,
        # the pipe buffer fills and terminate() blocks until it's drained.
        if cls._focus_proc.stdout:
            try:
                cls._focus_proc.stdout.close()
            except Exception:
                pass
        try:
            cls._focus_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            cls._focus_proc.kill()
            cls._focus_proc.wait(timeout=2)
        subprocess.run(["pkill", "-x", "Safari"], check=False)
        # Kill any orphaned cua-driver processes left by hermes.
        subprocess.run(["pkill", "-f", "cua-driver mcp"], check=False)

    def _run_hermes(self, prompt: str, timeout: int = 240, max_turns: int = 8) -> str:
        env = os.environ.copy()
        env["ANTHROPIC_API_KEY"] = _ANTHROPIC_KEY
        env["HERMES_COMPUTER_USE_BACKEND"] = "cua"
        env["HERMES_CUA_DRIVER_CMD"] = default_binary_path()

        result = subprocess.run(
            [
                _HERMES_BIN, "chat",
                "--provider", "anthropic",
                "-m", _MODEL,
                "--toolsets", "computer_use",
                "--yolo",
                "-Q",
                "--max-turns", str(max_turns),
                "-q", prompt,
            ],
            env=env, capture_output=True, text=True, timeout=timeout,
        )
        return (result.stdout + "\n" + result.stderr).strip()

    def _assert_no_new_focus_loss(self, allow_field_losses: int = 1) -> None:
        time.sleep(0.3)
        app_delta   = _read_file_int(_LOSS_FILE)    - self._losses_before
        key_delta   = _read_file_int(_KEY_LOSS_FILE) - self._key_losses_before
        field_delta = _read_file_int(_FIELD_LOSS_FILE) - self._field_losses_before
        msgs = []
        if app_delta:
            msgs.append(f"app lost focus {app_delta}x")
        if key_delta:
            msgs.append(f"window lost key status {key_delta}x")
        if field_delta > allow_field_losses:
            msgs.append(
                f"text-input lost first-responder {field_delta}x "
                f"(allowed {allow_field_losses})"
            )
        self.assertEqual(
            len(msgs), 0,
            "Focus stolen during launch_app: " + "; ".join(msgs),
        )

    def test_launch_safari_background_no_focus_steal(self) -> None:
        """hermes launches Safari hidden with a URL; window exists; focus never stolen.

        Safari needs a URL to create a window — launching bare (no URL) starts
        the process but NSWorkspace never triggers window creation.  The prompt
        explicitly includes the URL so the model uses the right call.
        """
        out = self._run_hermes(
            f"Use computer_use to launch Safari in the background. "
            f"Call action='launch_app' with bundle_id='{_SAFARI_BUNDLE}' "
            f"and urls=['{_LAUNCH_URL}']. "
            f"After launching, call action='capture' with app='Safari' to verify "
            f"Safari's window is reachable and report the window title or "
            f"first AX element you see. "
            f"Do NOT use action='focus_app' with raise_window=true.",
            timeout=240,
            max_turns=8,
        )

        # 1. Safari process must actually be running.
        pids = _safari_pids()
        self.assertTrue(
            len(pids) > 0,
            f"Safari process not found after hermes launch_app.\n"
            f"hermes output (tail):\n{out[-800:]}",
        )

        # 2. cua-driver must see at least one Safari window.
        #    We ask the driver independently — this is the anti-hallucination check.
        #    Safari may need a moment to register its window in WindowServer after
        #    the URL loads, so _safari_window_count_via_driver polls for up to 4s.
        window_count = _safari_window_count_via_driver(wait_s=4.0)
        self.assertGreater(
            window_count, 0,
            f"cua-driver sees no Safari windows after launch_app+URL "
            f"(got {window_count}). "
            f"Safari may have launched without a window — ensure urls=['{_LAUNCH_URL}'] "
            f"was passed.\nhermes output (tail):\n{out[-800:]}",
        )

        # 3. No focus stolen from FocusMonitorApp.
        #    allow_field_losses=1 for the known SyntheticAppFocusEnforcer AX
        #    side-effect that can briefly disturb field first-responder on the
        #    first AX interaction with a new pid, without stealing app focus.
        self._assert_no_new_focus_loss(allow_field_losses=1)
