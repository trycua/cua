"""Integration test: agent-cursor overlay z-ordering.

Verifies that after driving a backgrounded window the cua-driver overlay
is z-ordered JUST ABOVE the target window (NSWindowLevel.normal + ordered
above target) rather than at NSWindowLevel.floating (above ALL normal windows).

Expected ordering after pinning above a background window:
    [... , target-window , overlay , windows-that-were-above-target , ...]

Two assertions are verified:

1. **overlay appears at layer 0** — the overlay's CGWindowLayer is 0
   (NSWindowLevel.normal), so it shows up in list_windows. At .floating
   (level 3) it would be absent from the layer-0 filtered list.

2. **sandwich ordering** — overlay z_index > target z_index, and any
   window that was above the target BEFORE the click remains above the
   overlay AFTER the click.

Run:
    CUA_DRIVER_BINARY=.build/CuaDriver.app/Contents/MacOS/cua-driver \\
      python3 -m unittest test_overlay_z_order -v
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path, resolve_window_id  # noqa: E402

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))
_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_DIR, "FocusMonitorApp.app", "Contents", "MacOS", "FocusMonitorApp"
)

CALCULATOR_BUNDLE = "com.apple.calculator"


def _ensure_focus_app_built() -> None:
    src = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.swift")
    if not os.path.exists(_FOCUS_APP_EXE) or (
        os.path.exists(src) and os.path.getmtime(src) > os.path.getmtime(_FOCUS_APP_EXE)
    ):
        subprocess.run([os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True)


def _launch_focus_app() -> tuple[subprocess.Popen, int]:
    proc = subprocess.Popen(
        [_FOCUS_APP_EXE], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True
    )
    for _ in range(40):
        line = proc.stdout.readline().strip()
        if line.startswith("FOCUS_PID="):
            return proc, int(line.split("=", 1)[1])
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError("FocusMonitorApp did not print FOCUS_PID= in time")


class TestOverlayZOrder(unittest.TestCase):
    """Overlay is sandwiched just above the target window, not floating above all."""

    _focus_proc: subprocess.Popen
    _focus_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        _ensure_focus_app_built()
        # Kill Calculator if stale
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)

    @classmethod
    def tearDownClass(cls) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)

    def setUp(self) -> None:
        self.client = DriverClient(default_binary_path()).__enter__()
        # Ensure agent cursor is enabled for this test session.
        self.client.call_tool("set_agent_cursor_enabled", {"enabled": True})

        # Launch Calculator in background (it may briefly become frontmost).
        r = self.client.call_tool("launch_app", {"bundle_id": CALCULATOR_BUNDLE})
        self.calc_pid = r["structuredContent"]["pid"]
        time.sleep(0.5)

        # Launch FocusMonitorApp as frontmost so Calculator is definitely behind it.
        self._focus_proc, self._focus_pid = _launch_focus_app()
        time.sleep(0.5)  # let it settle as the frontmost window

    def tearDown(self) -> None:
        self._focus_proc.terminate()
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        self.client.__exit__(None, None, None)

    # ------------------------------------------------------------------

    def test_overlay_at_normal_level_above_target(self) -> None:
        """Overlay appears at layer=0 (not .floating/layer=3) and z_index > target."""
        # Snapshot all on-screen windows BEFORE the click.
        all_before = self._all_on_screen_windows()
        driver_pid = self.client.process.pid

        calc_wins_before = [w for w in all_before if w["pid"] == self.calc_pid]
        self.assertTrue(calc_wins_before, "Calculator window not visible before click")
        calc_z_before = max(w["z_index"] for w in calc_wins_before)
        calc_win_before = max(calc_wins_before, key=lambda w: w["z_index"])
        calc_win_id = calc_win_before["window_id"]

        # Use a pixel-addressed click at the center of the Calculator window
        # so we don't need get_window_state element indices.
        b = calc_win_before["bounds"]
        cx = b["width"] / 2.0
        cy = b["height"] / 2.0

        # Windows above Calculator that are NOT the driver overlay.
        fg_wins_before = [
            w for w in all_before
            if w["z_index"] > calc_z_before
            and w["pid"] != driver_pid
            and w.get("is_on_screen")
        ]

        # Trigger a pixel click to make the overlay appear and pin above Calculator.
        self.client.call_tool(
            "click",
            {
                "pid": self.calc_pid,
                "window_id": calc_win_id,
                "x": cx,
                "y": cy,
            },
        )

        # Let the defensive-repin ticks fully settle (last tick fires at ~1200ms
        # after the second pinAbove inside click, which returns after animations).
        time.sleep(1.5)

        # ── Assertion 1: overlay is visible at layer 0 ─────────────────────────
        all_after = self._all_on_screen_windows()
        overlay_wins = [w for w in all_after if w["pid"] == driver_pid]
        self.assertTrue(
            overlay_wins,
            f"cua-driver overlay NOT visible in list_windows at layer=0 "
            f"(driver pid={driver_pid}). "
            f"Expected NSWindowLevel.normal overlay to appear as a layer-0 window. "
            f"If level is still .floating (layer 3) it would be filtered out here.",
        )

        overlay_z = max(w["z_index"] for w in overlay_wins)

        # ── Assertion 2: overlay is above the target ────────────────────────────
        calc_wins_after = [w for w in all_after if w["pid"] == self.calc_pid]
        calc_z_after = (
            max(w["z_index"] for w in calc_wins_after) if calc_wins_after else 0
        )
        self.assertGreater(
            overlay_z,
            calc_z_after,
            f"overlay (z={overlay_z}) must be ABOVE Calculator (z={calc_z_after}). "
            f"pinAbove must order the overlay at z_target+1.",
        )

        # ── Assertion 3: foreground windows remain above the overlay ────────────
        # Any window that was above Calculator BEFORE the click and is still
        # on-screen AFTER the click must have z_index > overlay_z.
        # This is the key invariant that distinguishes .normal from .floating.
        missed_above = []
        for w_before in fg_wins_before:
            w_after = next(
                (a for a in all_after if a["window_id"] == w_before["window_id"]),
                None,
            )
            if w_after and w_after.get("is_on_screen"):
                if w_after["z_index"] <= overlay_z:
                    missed_above.append(
                        f"{w_before['app_name']} "
                        f"(window_id={w_before['window_id']}, "
                        f"z_before={w_before['z_index']}, "
                        f"z_after={w_after['z_index']}, overlay_z={overlay_z})"
                    )

        if fg_wins_before and missed_above:
            self.fail(
                "Window(s) that were ABOVE Calculator before the click are now "
                "BELOW OR EQUAL TO the overlay — overlay is NOT sandwiched:\n"
                + "\n".join(f"  {m}" for m in missed_above)
                + "\nExpected ordering: [target, overlay, fg-windows]. "
                "This means the overlay is still at .floating level instead of .normal."
            )

        # If there were no foreground windows above Calculator, just note it.
        if not fg_wins_before:
            # Calculator was already frontmost; z-sandwich can't be verified.
            # Assertion 1 + 2 are sufficient in this case.
            pass

    # ------------------------------------------------------------------

    def _all_on_screen_windows(self) -> list[dict]:
        result = self.client.call_tool("list_windows", {"on_screen_only": True})
        return result["structuredContent"]["windows"]


if __name__ == "__main__":
    unittest.main(verbosity=2)
