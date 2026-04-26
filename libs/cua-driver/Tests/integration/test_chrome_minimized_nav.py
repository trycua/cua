"""Integration test: Chrome minimized-window navigation deminiaturizes unavoidably.

Reproduces the bug: driving URL navigation on a minimized Chrome
window always causes Chrome to deminiaturize.

Setup:
  1. Launch Chrome, navigate to about:blank, then minimize it.
  2. Launch FocusMonitorApp as the foreground witness.

Tests verify that various interaction paths either:
  - Preserve minimized state + don't steal focus (green), OR
  - Cause deminiaturize / focus steal (expected failures documenting the bug).

Once the bug is fixed, all tests should pass.

Run:
    scripts/test.sh test_chrome_minimized_nav
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

CHROME_BUNDLE = "com.google.Chrome"
FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

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


def _find_element_index(tree: str, label: str) -> int | None:
    for line in tree.split("\n"):
        if label in line:
            m = re.search(r'\[(\d+)\]', line)
            if m:
                return int(m.group(1))
    return None


def _chrome_is_minimized(client: DriverClient, pid: int) -> bool:
    """Check if Chrome's main window is minimized by looking for AXMinimized."""
    window_id = resolve_window_id(client, pid, require_on_current_space=False)
    snap = client.call_tool(
        "get_window_state", {"pid": pid, "window_id": window_id}
    )
    tree = snap.get("structuredContent", snap).get("tree_markdown", "")
    # A minimized window typically won't have on-screen content,
    # but we can check the has_screenshot field — if no on-screen
    # window, screenshot will be absent.
    has_shot = snap.get("structuredContent", snap).get("has_screenshot", False)
    return not has_shot


def _activate_focus_monitor() -> None:
    subprocess.run(
        ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
        check=False,
    )
    time.sleep(0.5)


# ---------------------------------------------------------------------------
# Test class
# ---------------------------------------------------------------------------

class ChromeMinimizedNavTests(unittest.TestCase):
    """Interact with a minimized Chrome without deminiaturizing or stealing focus."""

    _chrome_pid: int
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

        # Find or launch Chrome.
        with DriverClient(cls.binary) as c:
            apps = c.call_tool("list_apps")["structuredContent"]["apps"]
            chrome = [a for a in apps if a.get("bundle_id") == CHROME_BUNDLE]
            if chrome:
                cls._chrome_pid = chrome[0]["pid"]
            else:
                result = c.call_tool("launch_app", {
                    "bundle_id": CHROME_BUNDLE,
                    "urls": ["about:blank"],
                })
                cls._chrome_pid = result["structuredContent"]["pid"]
                time.sleep(2.0)
        print(f"\n  Chrome pid: {cls._chrome_pid}")

        # Ensure Chrome has an about:blank window via launch_app (no focus steal).
        with DriverClient(cls.binary) as c:
            c.call_tool("launch_app", {
                "bundle_id": CHROME_BUNDLE,
                "urls": ["about:blank"],
            })
        time.sleep(2.0)

        # Snapshot AX tree BEFORE minimizing to cache the omnibox index.
        # Cache the window_id too — element indices are keyed on
        # (pid, window_id) and subsequent calls need the same pair to
        # resolve cached indices.
        with DriverClient(cls.binary) as c:
            cls._chrome_window_id = resolve_window_id(c, cls._chrome_pid)
            snap = c.call_tool("get_window_state", {
                "pid": cls._chrome_pid,
                "window_id": cls._chrome_window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            cls._omnibox_idx = _find_element_index(tree, "Address and search bar")
            if cls._omnibox_idx is None:
                cls._omnibox_idx = _find_element_index(tree, "address_bar")
        print(f"  Omnibox element_index (pre-minimize): {cls._omnibox_idx}")
        print(f"  Chrome window_id: {cls._chrome_window_id}")

        # Minimize Chrome via Cmd+M.
        with DriverClient(cls.binary) as c:
            c.call_tool("hotkey", {
                "pid": cls._chrome_pid,
                "keys": ["cmd", "m"],
            })
        time.sleep(1.5)
        print("  Chrome minimized via Cmd+M")

        # Launch FocusMonitorApp (becomes frontmost).
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
        # Close the Chrome window we opened.
        subprocess.run(
            ["osascript", "-e",
             'tell application "Google Chrome" to close window 1'],
            check=False,
        )
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    def setUp(self) -> None:
        # Minimize Chrome via hotkey — cua-driver delivers Cmd+M to the pid
        # via CGEvent.postToPid without stealing focus.
        with DriverClient(self.binary) as c:
            c.call_tool("hotkey", {
                "pid": self._chrome_pid,
                "keys": ["cmd", "m"],
            })
        time.sleep(1.5)
        _activate_focus_monitor()
        self._losses_before = _read_focus_losses()

    def _assert_focus_preserved(self, label: str) -> None:
        """FocusMonitorApp is still frontmost."""
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

    def _assert_chrome_still_minimized(self, label: str) -> None:
        """Chrome window is still minimized — verified by checking
        if get_window_state returns has_screenshot=false (no on-screen window)."""
        with DriverClient(self.binary) as c:
            snap = c.call_tool("get_window_state", {
                "pid": self._chrome_pid,
                "window_id": self._chrome_window_id,
            })
            sc = snap.get("structuredContent", snap)
            has_shot = sc.get("has_screenshot", False)
        is_minimized = not has_shot
        print(f"  [{label}] Chrome minimized (no screenshot): {is_minimized}")
        self.assertTrue(
            is_minimized,
            f"[{label}] Chrome deminiaturized (has_screenshot={has_shot})",
        )

    # -- Test 1: get_window_state should be safe (read-only) ---------------

    def test_01_get_window_state_safe(self) -> None:
        """AX snapshot of minimized Chrome should not deminiaturize."""
        with DriverClient(self.binary) as c:
            snap = c.call_tool("get_window_state", {
                "pid": self._chrome_pid,
                "window_id": self._chrome_window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"\n  tree length: {len(tree)} chars")

        self._assert_chrome_still_minimized("01_get_window_state")
        self._assert_focus_preserved("01_get_window_state")

    # -- Test 2: AX click on omnibox deminiaturizes (documents the bug) ----

    def test_02_ax_click_omnibox(self) -> None:
        """AX-click the omnibox in minimized Chrome — should block with error, not deminiaturize."""
        idx = self._omnibox_idx
        print(f"\n  omnibox element_index (cached): {idx}")
        if idx is None:
            self.skipTest("Omnibox not found in pre-minimize AX tree")

        with DriverClient(self.binary) as c:
            # Refresh element cache.
            c.call_tool("get_window_state", {
                "pid": self._chrome_pid,
                "window_id": self._chrome_window_id,
            })
            result = c.call_tool("click", {
                "pid": self._chrome_pid,
                "window_id": self._chrome_window_id,
                "element_index": idx,
            })
            text = ""
            for item in result.get("content", []):
                if item.get("type") == "text":
                    text = item.get("text", "")
            print(f"\n  result: {text[:120]}")

        time.sleep(0.5)
        self._assert_chrome_still_minimized("02_ax_click_omnibox")
        self._assert_focus_preserved("02_ax_click_omnibox")

    # -- Test 3: set_value on omnibox deminiaturizes (documents the bug) ---

    def test_03_set_value_omnibox(self) -> None:
        """Set AX value on the omnibox in minimized Chrome — should block with error, not deminiaturize."""
        idx = self._omnibox_idx
        print(f"\n  omnibox element_index (cached): {idx}")
        if idx is None:
            self.skipTest("Omnibox not found in pre-minimize AX tree")

        with DriverClient(self.binary) as c:
            c.call_tool("get_window_state", {
                "pid": self._chrome_pid,
                "window_id": self._chrome_window_id,
            })
            result = c.call_tool("set_value", {
                "pid": self._chrome_pid,
                "window_id": self._chrome_window_id,
                "element_index": idx,
                "value": "https://example.com",
            })
            text = ""
            for item in result.get("content", []):
                if item.get("type") == "text":
                    text = item.get("text", "")
            print(f"\n  result: {text[:120]}")

        time.sleep(0.5)
        self._assert_chrome_still_minimized("03_set_value_omnibox")
        self._assert_focus_preserved("03_set_value_omnibox")

    # -- Test 4: type_text_chars without omnibox focus — stays minimized ---

    def test_04_type_without_omnibox_focus(self) -> None:
        """Type keys to minimized Chrome without focusing omnibox — should stay minimized."""
        with DriverClient(self.binary) as c:
            c.call_tool("type_text_chars", {
                "pid": self._chrome_pid,
                "text": "hello",
            })

        time.sleep(0.5)
        self._assert_chrome_still_minimized("04_type_no_omnibox")
        self._assert_focus_preserved("04_type_no_omnibox")

    # -- Test 5: FocusWithoutRaise — stays minimized -----------------------

    def test_05_focus_without_raise_safe(self) -> None:
        """FocusWithoutRaise on minimized Chrome — should stay minimized.

        This tests the yabai-derived SLPS focus recipe that puts
        Chrome into AppKit-active state without raising windows.
        """
        with DriverClient(self.binary) as c:
            # get_window_state uses FocusWithoutRaise internally for
            # AX enablement. Just verify the snapshot path is safe.
            snap = c.call_tool("get_window_state", {
                "pid": self._chrome_pid,
                "window_id": self._chrome_window_id,
            })
            count = snap.get("structuredContent", snap).get("element_count", 0)
            print(f"\n  element_count: {count}")

        self._assert_chrome_still_minimized("05_focus_without_raise")
        self._assert_focus_preserved("05_focus_without_raise")


if __name__ == "__main__":
    unittest.main(verbosity=2)
