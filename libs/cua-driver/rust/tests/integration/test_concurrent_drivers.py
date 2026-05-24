"""Integration test: two cua-driver processes running concurrently.

Verifies that two independent cua-driver instances can coexist without:
  - crashing each other
  - stealing focus from FocusMonitorApp
  - interfering with each other's operations

Scenario:
  Driver A  → AX-clicks Calculator (2 + 2 = 4)
  Driver B  → Concurrently moves its agent cursor and inspects window list

Run:
    CUA_DRIVER_BINARY=../../target/debug/cua-driver python3 -m unittest test_concurrent_drivers -v
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import threading
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


class ConcurrentDriversTest(unittest.TestCase):
    """Two driver processes running side-by-side without interference."""

    _calc_pid: int
    _focus_proc: subprocess.Popen
    _focus_pid: int
    binary: str

    @classmethod
    def setUpClass(cls) -> None:
        _build_focus_app()

        # Kill stale processes from any prior crashed run.
        subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)

        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

        cls.binary = default_binary_path()

        # Launch Calculator via Driver A.
        with DriverClient(cls.binary) as c:
            result = c.call_tool("launch_app", {"bundle_id": CALC_BUNDLE})
            cls._calc_pid = result["structuredContent"]["pid"]
        print(f"\n  Calculator pid: {cls._calc_pid}")
        time.sleep(1.5)

        # Launch FocusMonitorApp (becomes frontmost).
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
            if cls._focus_proc.stdout:
                cls._focus_proc.stdout.close()
        subprocess.run(["pkill", "-x", "FocusMonitorApp"], check=False)
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    def setUp(self) -> None:
        # Clear Calculator before each test so previous display state doesn't interfere.
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn_ac = _find_calc_button(tree, "All Clear") or _find_calc_button(tree, "Clear")
            if btn_ac is not None:
                c.call_tool("click", {
                    "pid": self._calc_pid, "window_id": window_id, "element_index": btn_ac,
                })
                time.sleep(0.2)

        subprocess.run(
            ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
            check=False,
        )
        time.sleep(0.5)
        self._losses_before = _read_focus_losses()

    # ── tests ─────────────────────────────────────────────────────────────────

    def test_two_drivers_run_concurrently(self) -> None:
        """Open Driver A and Driver B simultaneously; both must succeed without crashing."""

        driver_a_result: dict = {}
        driver_b_result: dict = {}
        driver_a_error: list[str] = []
        driver_b_error: list[str] = []

        def run_driver_a() -> None:
            try:
                with DriverClient(self.binary) as ca:
                    # Driver A: AX-click 2 + 2 = on Calculator.
                    window_id = resolve_window_id(ca, self._calc_pid)
                    snap = ca.call_tool("get_window_state", {
                        "pid": self._calc_pid, "window_id": window_id,
                    })
                    tree = snap.get("structuredContent", snap).get("tree_markdown", "")
                    btn_2 = _find_calc_button(tree, "2")
                    btn_add = _find_calc_button(tree, "Add")
                    btn_eq = _find_calc_button(tree, "Equals")
                    if None in (btn_2, btn_add, btn_eq):
                        driver_a_error.append("Missing calculator buttons")
                        return
                    for idx in [btn_2, btn_add, btn_2, btn_eq]:
                        ca.call_tool("click", {
                            "pid": self._calc_pid,
                            "window_id": window_id,
                            "element_index": idx,
                        })
                        time.sleep(0.15)
                    snap2 = ca.call_tool("get_window_state", {
                        "pid": self._calc_pid, "window_id": window_id,
                        "query": "AXStaticText",
                    })
                    tree2 = snap2.get("structuredContent", snap2).get("tree_markdown", "")
                    driver_a_result["tree"] = tree2
            except Exception as e:
                driver_a_error.append(str(e))

        def run_driver_b() -> None:
            try:
                with DriverClient(self.binary) as cb:
                    # Driver B: move its cursor, list windows, move cursor again.
                    cb.call_tool("move_cursor", {"x": 200.0, "y": 200.0, "cursor_id": "agent-b"})
                    wins = cb.call_tool("list_windows", {"on_screen_only": True})
                    count = len(wins.get("structuredContent", {}).get("windows", []))
                    cb.call_tool("move_cursor", {"x": 300.0, "y": 300.0, "cursor_id": "agent-b"})
                    driver_b_result["window_count"] = count
            except Exception as e:
                driver_b_error.append(str(e))

        # Launch both threads simultaneously.
        ta = threading.Thread(target=run_driver_a, daemon=True)
        tb = threading.Thread(target=run_driver_b, daemon=True)
        ta.start()
        tb.start()
        ta.join(timeout=30)
        tb.join(timeout=30)

        # Both must have completed without error.
        self.assertFalse(
            driver_a_error,
            f"Driver A errored: {driver_a_error}",
        )
        self.assertFalse(
            driver_b_error,
            f"Driver B errored: {driver_b_error}",
        )

        print(f"\n  Driver A result tree:\n{driver_a_result.get('tree', '(empty)')}")
        print(f"  Driver B window count: {driver_b_result.get('window_count', -1)}")

        # Driver A should have produced "4".
        self.assertIn(
            "4",
            driver_a_result.get("tree", ""),
            "Calculator did not show 4 after Driver A clicked 2+2=",
        )

        # Driver B should have seen at least some windows.
        self.assertGreater(
            driver_b_result.get("window_count", 0),
            0,
            "Driver B saw no windows in list_windows",
        )

        # Focus must not have been stolen.
        time.sleep(0.3)
        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        print(f"  losses: {self._losses_before}->{losses}, frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen during concurrent operation — frontmost is {active}",
        )

    def test_sequential_driver_reuse(self) -> None:
        """Open/close/reopen a DriverClient — state from prior session does not bleed."""

        # First session — clear Calculator.
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn_ac = _find_calc_button(tree, "All Clear") or _find_calc_button(tree, "Clear")
            if btn_ac is not None:
                c.call_tool("click", {
                    "pid": self._calc_pid, "window_id": window_id, "element_index": btn_ac,
                })
                time.sleep(0.2)

        time.sleep(0.2)

        # Second session — press 5.
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn_5 = _find_calc_button(tree, "5")
            self.assertIsNotNone(btn_5, "Could not find '5' button in fresh session")
            c.call_tool("click", {
                "pid": self._calc_pid, "window_id": window_id, "element_index": btn_5,
            })
            time.sleep(0.3)
            snap2 = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id, "query": "AXStaticText",
            })
            tree2 = snap2.get("structuredContent", snap2).get("tree_markdown", "")
            print(f"\n  After pressing 5: {tree2[:200]}")

        self.assertIn("5", tree2, "Calculator did not show 5 after clicking it in a new session")

        # Focus check.
        time.sleep(0.3)
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"Focus stolen — frontmost is {active}",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
