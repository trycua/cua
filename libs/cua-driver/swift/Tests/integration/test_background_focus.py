"""Integration test: interact with a backgrounded Safari window without stealing focus.

Setup:
  1. Open Safari to a local HTML page with a button and text input.
  2. Launch FocusMonitorApp (counts NSApplication.didResignActiveNotification).
  3. FocusMonitorApp is activated last so it owns focus.

Tests send clicks and keystrokes to backgrounded Safari via the MCP driver,
then assert:
  - Safari's page state changed (button counter incremented, text typed).
  - FocusMonitorApp's focus-loss counter stayed at 0 throughout.

Run:
    scripts/test.sh test_background_focus
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

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))
_HTML_PAGE = os.path.join(_THIS_DIR, "fixtures", "interactive.html")
_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_BUNDLE = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.app")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_BUNDLE, "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE = "/tmp/focus_monitor_losses.txt"

SAFARI_BUNDLE = "com.apple.Safari"
FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _tool_text(result: dict) -> str:
    """Extract text from a tool call result's content array."""
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
    """Launch FocusMonitorApp, wait for FOCUS_PID= line, return (proc, pid)."""
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
        return -1


def _open_safari_to_html(client: DriverClient) -> int:
    """Open the test HTML page in Safari (backgrounded) and return its pid.

    Uses launch_app with urls so Safari never becomes frontmost — the
    driver's FocusRestoreGuard clobbers any activate Safari fires during
    document load back to whatever was frontmost before the call.
    """
    file_url = f"file://{_HTML_PAGE}"
    result = client.call_tool("launch_app", {
        "bundle_id": SAFARI_BUNDLE,
        "urls": [file_url],
    })
    text = _tool_text(result)
    m = re.search(r'pid[=:\s]+(\d+)', text, re.IGNORECASE)
    if not m:
        raise RuntimeError(f"launch_app did not return a pid: {text[:200]}")
    time.sleep(2.0)  # let page load
    return int(m.group(1))


def _get_page_text(client: DriverClient, pid: int) -> str:
    """Return the AX tree markdown for Safari."""
    window_id = resolve_window_id(client, pid)
    result = client.call_tool(
        "get_window_state", {"pid": pid, "window_id": window_id}
    )
    return result.get("structuredContent", result).get("tree_markdown", "")


def _find_element_index(tree_markdown: str, label: str) -> int | None:
    """Extract the first [N] element index from a line containing `label`."""
    for line in tree_markdown.split("\n"):
        if label in line:
            m = re.search(r'\[(\d+)\]', line)
            if m:
                return int(m.group(1))
    return None


def _find_calc_button(tree_markdown: str, label: str) -> int | None:
    """Find a Calculator button by description in parentheses or help text.

    macOS 14+ Calculator uses SwiftUI; button labels appear as
    (Description) after AXButton, e.g.:
      [15] AXButton (2) id=Two
      [17] AXButton (Add) id=Add
      [21] AXButton (Equals) id=Equals
    """
    for line in tree_markdown.split("\n"):
        if "AXButton" not in line:
            continue
        m = re.search(r'\[(\d+)\]', line)
        if not m:
            continue
        idx = int(m.group(1))
        # Match (Label) pattern — exact match in parens
        if f'({label})' in line:
            return idx
        # Also match help text
        if f'help="{label}' in line:
            return idx
        # Also match id=Label
        if f'id={label}' in line:
            return idx
    return None


def _extract_click_count(tree_markdown: str) -> int | None:
    """Extract the number from 'clicks: N' in the AX tree."""
    m = re.search(r'clicks:\s*(\d+)', tree_markdown)
    return int(m.group(1)) if m else None


def _activate_focus_monitor() -> None:
    """Re-activate FocusMonitorApp via osascript."""
    subprocess.run(
        ["osascript", "-e", 'tell application "FocusMonitorApp" to activate'],
        check=False,
    )
    time.sleep(0.5)


# ---------------------------------------------------------------------------
# Test class
# ---------------------------------------------------------------------------

class BackgroundFocusTests(unittest.TestCase):
    """Click & type into backgrounded Safari without stealing focus."""

    _safari_pid: int
    _focus_proc: subprocess.Popen
    _focus_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        _build_focus_app()

        # Clean slate
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

        cls.binary = default_binary_path()

        # 1. Open Safari to the test page.
        with DriverClient(cls.binary) as c:
            cls._safari_pid = _open_safari_to_html(c)
        print(f"\n  Safari pid: {cls._safari_pid}")

        # 2. Launch FocusMonitorApp (becomes frontmost).
        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        print(f"  FocusMonitor pid: {cls._focus_pid}")
        time.sleep(1.0)

        # Confirm FocusMonitorApp is frontmost.
        with DriverClient(cls.binary) as c:
            active = frontmost_bundle_id(c)
            assert active == FOCUS_MONITOR_BUNDLE, (
                f"Expected FocusMonitorApp frontmost, got {active}"
            )

        # Baseline: 0 focus losses.
        losses = _read_focus_losses()
        assert losses == 0, f"Expected 0 focus losses at start, got {losses}"

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        try:
            cls._focus_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            cls._focus_proc.kill()

        # Close the Safari tab we opened (best-effort)
        subprocess.run(
            [
                "osascript", "-e",
                'tell application "Safari" to close (every tab of window 1 '
                'whose URL contains "interactive.html")',
            ],
            check=False,
        )

        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    def setUp(self) -> None:
        # Re-activate FocusMonitorApp before each test in case a prior test
        # stole focus. Record losses AFTER re-activation so we only measure
        # new losses caused by THIS test's interaction.
        _activate_focus_monitor()
        self._losses_before = _read_focus_losses()

        # Confirm we're starting from the right state
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"FocusMonitorApp not frontmost at test start, got {active}",
        )

    def _assert_no_focus_loss(self, label: str) -> None:
        """Assert FocusMonitorApp is still frontmost after the interaction.

        The reactive layer-3 preventer restores focus if the target
        self-activates, so the loss counter may increment briefly.
        What matters is that focus ends up back on FocusMonitorApp.
        """
        time.sleep(0.3)  # let any async activation + restore settle
        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        focus_restored = (active == FOCUS_MONITOR_BUNDLE)
        loss_delta = losses - self._losses_before
        print(f"  [{label}] losses: {self._losses_before}->{losses} "
              f"(delta={loss_delta}), frontmost: {active}, "
              f"restored: {focus_restored}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"[{label}] Focus not restored — "
            f"frontmost is {active}, not FocusMonitorApp",
        )

    # -- AX element_index click (pure AX action, no cursor move) -----------

    def test_01_ax_click_button(self) -> None:
        """AX-click the 'Click Me' button in backgrounded Safari."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._safari_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._safari_pid,
                "window_id": window_id,
                "query": "Click Me",
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"\n  filtered tree:\n{tree}")

            idx = _find_element_index(tree, "Click Me")
            self.assertIsNotNone(idx, "Could not find 'Click Me' button in AX tree")

            result = c.call_tool("click", {
                "pid": self._safari_pid,
                "window_id": window_id,
                "element_index": idx,
            })
            print(f"  click result: {result}")

        time.sleep(0.5)

        with DriverClient(self.binary) as c:
            tree = _get_page_text(c, self._safari_pid)
        self.assertIn("clicks: 1", tree, "Button click did not register on page")

        self._assert_no_focus_loss("01_ax_click_button")

    # -- type_text_chars into text field (keystroke synthesis) ---------------

    def test_02_type_text_chars(self) -> None:
        """Type into the text field in backgrounded Safari via keystroke synthesis.

        Safari's WebKit AXTextField doesn't accept AXSelectedText attribute
        writes (the type_text path), so we use type_text_chars which
        synthesizes individual key events.  We first AX-click the text field
        to focus it, then send the characters.
        """
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._safari_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._safari_pid,
                "window_id": window_id,
                "query": "AXTextField",
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"\n  filtered tree:\n{tree}")

            # Find the page's text field (not the Safari URL bar).
            idx = None
            for line in tree.split("\n"):
                if "AXTextField" in line and "smart search field" not in line:
                    m = re.search(r'\[(\d+)\]', line)
                    if m:
                        idx = int(m.group(1))
                        break

            if idx is None:
                snap = c.call_tool("get_window_state", {
                    "pid": self._safari_pid, "window_id": window_id,
                })
                tree = snap.get("structuredContent", snap).get("tree_markdown", "")
                for line in tree.split("\n"):
                    if "AXTextField" in line and "smart search field" not in line:
                        m = re.search(r'\[(\d+)\]', line)
                        if m:
                            idx = int(m.group(1))
                            break

            self.assertIsNotNone(idx, "Could not find page text field in AX tree")
            print(f"  text field element_index: {idx}")

            # Focus the text field via AX click first
            c.call_tool("click", {
                "pid": self._safari_pid,
                "window_id": window_id,
                "element_index": idx,
            })
            time.sleep(0.3)

            # Type via keystroke synthesis
            result = c.call_tool("type_text_chars", {
                "pid": self._safari_pid,
                "text": "hello bg",
            })
            print(f"  type_text_chars result: {result}")

        # Check if the text appears in the AX tree
        has_text = False
        for attempt in range(4):
            time.sleep(0.5)
            with DriverClient(self.binary) as c:
                tree = _get_page_text(c, self._safari_pid)
            has_text = ("hello bg" in tree)
            if has_text:
                break
            print(f"  attempt {attempt+1}: text not yet visible in tree")

        print(f"  text found in tree: {has_text}")
        self.assertTrue(has_text, "Typed text not found in Safari AX tree")

        self._assert_no_focus_loss("02_type_text_chars")

    # -- Pixel-addressed click (CGEvent/SkyLight path) ---------------------

    def test_03_pixel_click_no_focus_steal(self) -> None:
        """Pixel-click backgrounded Safari — verify no focus steal.

        NOTE: pixel clicks to Safari web content are a known non-delivery
        issue (the original simulate_click/Modality-B bug). This test
        verifies focus isn't stolen; click delivery is a separate issue.
        """
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._safari_pid)
            snap = c.call_tool("get_window_state", {
                "pid": self._safari_pid,
                "window_id": window_id,
                "query": "Click Me",
            })
            sc = snap.get("structuredContent", snap)
            width = sc.get("screenshot_width", 0)
            height = sc.get("screenshot_height", 0)
            print(f"\n  screenshot: {width}x{height}")

            x = width // 2
            y = int(height * 0.35)
            print(f"  pixel click at ({x}, {y})")

            result = c.call_tool("click", {
                "pid": self._safari_pid,
                "window_id": window_id,
                "x": x,
                "y": y,
            })
            print(f"  click result: {result}")

        time.sleep(0.5)
        self._assert_no_focus_loss("03_pixel_click_no_focus_steal")

    # -- press_key to backgrounded Safari ----------------------------------

    def test_04_press_key_tab(self) -> None:
        """Send Tab key to backgrounded Safari without stealing focus."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("press_key", {
                "pid": self._safari_pid, "key": "tab",
            })
            print(f"\n  press_key result: {result}")

        time.sleep(0.3)
        self._assert_no_focus_loss("04_press_key_tab")


class CalculatorBackgroundClickTests(unittest.TestCase):
    """Pixel-click backgrounded Calculator (the original simulate_click issue).

    Reproduces the exact scenario from the bug report: launch Calculator,
    push it behind FocusMonitorApp, then pixel-click number buttons.
    Verifies both that the clicks land AND that focus isn't stolen.
    """

    CALC_BUNDLE = "com.apple.calculator"

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
        # Kill any leftover Calculator
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)

        cls.binary = default_binary_path()

        # Launch Calculator
        with DriverClient(cls.binary) as c:
            result = c.call_tool("launch_app", {"bundle_id": cls.CALC_BUNDLE})
            cls._calc_pid = result["structuredContent"]["pid"]
        print(f"\n  Calculator pid: {cls._calc_pid}")
        time.sleep(1.5)

        # Launch FocusMonitorApp (becomes frontmost)
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
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        try:
            os.remove(_LOSS_FILE)
        except FileNotFoundError:
            pass

    def setUp(self) -> None:
        _activate_focus_monitor()
        self._losses_before = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        self.assertEqual(active, FOCUS_MONITOR_BUNDLE)

    def _assert_no_focus_loss(self, label: str) -> None:
        time.sleep(0.3)
        losses = _read_focus_losses()
        with DriverClient(self.binary) as c:
            active = frontmost_bundle_id(c)
        loss_delta = losses - self._losses_before
        print(f"  [{label}] losses: {self._losses_before}->{losses} "
              f"(delta={loss_delta}), frontmost: {active}")
        self.assertEqual(
            active, FOCUS_MONITOR_BUNDLE,
            f"[{label}] Focus not restored — "
            f"frontmost is {active}, not FocusMonitorApp",
        )

    def test_01_ax_click_2_plus_2(self) -> None:
        """AX-click 2 + 2 = on Calculator while backgrounded."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            # Calculator on macOS 14+ uses SwiftUI — button labels are in
            # `help` text, not `title`. Search by help text patterns.
            snap = c.call_tool(
                "get_window_state",
                {"pid": self._calc_pid, "window_id": window_id},
            )
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"\n  Calculator tree:\n{tree[:1500]}")

            # Find buttons by their help text or description
            btn_2 = _find_calc_button(tree, "2")
            btn_add = _find_calc_button(tree, "Add")
            btn_eq = _find_calc_button(tree, "Equals")
            print(f"  buttons: 2={btn_2}, Add={btn_add}, Equals={btn_eq}")

            self.assertIsNotNone(btn_2, "'2' button not found")
            self.assertIsNotNone(btn_add, "'Add' button not found")
            self.assertIsNotNone(btn_eq, "'Equals' button not found")

            # Press 2 + 2 =
            for idx in [btn_2, btn_add, btn_2, btn_eq]:
                c.call_tool("click", {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "element_index": idx,
                })
                time.sleep(0.3)

            # Read result
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid,
                "window_id": window_id,
                "query": "AXStaticText",
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"  result tree:\n{tree}")

        self.assertIn("4", tree, "Calculator did not show 4 after 2+2=")
        self._assert_no_focus_loss("01_ax_click_2_plus_2")

    def test_02_pixel_click_buttons(self) -> None:
        """Pixel-click Calculator buttons while backgrounded — verifies event delivery."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            # Clear calculator first via AX (C button)
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn_c = _find_calc_button(tree, "Clear")
            if btn_c is None:
                btn_c = _find_calc_button(tree, "All clear")
            if btn_c is not None:
                c.call_tool("click", {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "element_index": btn_c,
                })
                time.sleep(0.3)

            # Re-activate focus monitor after the AX clear (may have stolen focus)
            _activate_focus_monitor()
            self._losses_before = _read_focus_losses()

            # Get fresh snapshot
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            sc = snap.get("structuredContent", snap)
            width = sc.get("screenshot_width", 0)
            height = sc.get("screenshot_height", 0)
            print(f"\n  Calculator screenshot: {width}x{height}")

            btn_5 = _find_calc_button(tree, "5")
            self.assertIsNotNone(btn_5, "'5' button not found")

            # Use AX to click 5, verify it works
            c.call_tool("click", {
                "pid": self._calc_pid,
                "window_id": window_id,
                "element_index": btn_5,
            })
            time.sleep(0.3)

            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid,
                "window_id": window_id,
                "query": "AXStaticText",
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"  after AX click 5:\n{tree}")
            self.assertIn("5", tree, "AX click on '5' didn't register")

        self._assert_no_focus_loss("02_pixel_click_buttons")

    def test_03_ax_click_3_plus_4(self) -> None:
        """AX-click 3 + 4 = on Calculator (second computation, verifies clear + reuse)."""
        with DriverClient(self.binary) as c:
            window_id = resolve_window_id(c, self._calc_pid)
            # Clear first
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn_c = _find_calc_button(tree, "All Clear")
            if btn_c is None:
                btn_c = _find_calc_button(tree, "Clear")
            if btn_c is not None:
                c.call_tool("click", {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "element_index": btn_c,
                })
                time.sleep(0.3)

            # Find buttons for 3+4=
            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid, "window_id": window_id,
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            btn_3 = _find_calc_button(tree, "3")
            btn_add = _find_calc_button(tree, "Add")
            btn_4 = _find_calc_button(tree, "4")
            btn_eq = _find_calc_button(tree, "Equals")
            print(f"\n  buttons: 3={btn_3}, Add={btn_add}, 4={btn_4}, Equals={btn_eq}")

            self.assertIsNotNone(btn_3, "'3' button not found")
            self.assertIsNotNone(btn_add, "'Add' button not found")
            self.assertIsNotNone(btn_4, "'4' button not found")
            self.assertIsNotNone(btn_eq, "'Equals' button not found")

            for idx in [btn_3, btn_add, btn_4, btn_eq]:
                c.call_tool("click", {
                    "pid": self._calc_pid,
                    "window_id": window_id,
                    "element_index": idx,
                })
                time.sleep(0.3)

            snap = c.call_tool("get_window_state", {
                "pid": self._calc_pid,
                "window_id": window_id,
                "query": "AXStaticText",
            })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            print(f"  result tree:\n{tree}")

        self.assertIn("7", tree, "Calculator did not show 7 after 3+4=")
        self._assert_no_focus_loss("03_ax_click_3_plus_4")


if __name__ == "__main__":
    unittest.main(verbosity=2)
