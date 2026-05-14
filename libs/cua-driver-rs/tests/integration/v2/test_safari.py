"""Safari background automation tests — v2 harness.

Tests interact with Safari via the public MCP interface only:
  get_window_state → click / type_text / press_key → get_window_state → assert

The driver picks the best internal technique (AX, CGEvent, SkyLight, etc.).
Tests verify observable DOM outcomes via the AX tree.

Setup: Safari is opened to test_page.html (served locally), then
FocusMonitorApp is activated so it owns the screen focus. Every test
asserts that focus never returned to Safari during the interaction.
"""

from __future__ import annotations

import subprocess
import sys
import time
import os

import re
import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)
from harness.driver import Driver
from harness import tree as Tree


def _parse_clicks(tree_text: str) -> int:
    """Extract the 'clicks: N' counter value from the AX tree."""
    m = re.search(r'clicks:\s*(\d+)', tree_text)
    return int(m.group(1)) if m else 0

SAFARI_BUNDLE = "com.apple.Safari"
FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"


# ── module-level Safari setup ─────────────────────────────────────────────────

@pytest.fixture(scope="module")
def safari_pid(binary, html_server, focus_monitor):
    """Open Safari to test_page.html; return its pid.

    Activated after Safari so FocusMonitorApp is frontmost during the tests.
    """
    subprocess.run(["pkill", "-x", "Safari"], check=False)
    time.sleep(0.5)

    page_url = f"{html_server}/test_page.html"
    subprocess.run(["open", "-g", "-a", "Safari", page_url], check=True)
    time.sleep(3.5)  # Safari launch + page load

    with Driver(binary) as d:
        app = d.find_app(SAFARI_BUNDLE)
        if app is None:
            pytest.fail("Safari did not appear in list_apps after launch")
        pid = app["pid"]

    print(f"\n  Safari pid: {pid}, page: {page_url}")
    yield pid

    subprocess.run(["pkill", "-x", "Safari"], check=False)


@pytest.fixture(autouse=True)
def _reactivate_focus(focus_monitor):
    """Restore FocusMonitorApp as frontmost before each test (by pid)."""
    _, pid = focus_monitor
    subprocess.run(
        ["osascript", "-e",
         f'tell application "System Events" to set frontmost of (first process whose unix id is {pid}) to true'],
        check=False,
    )
    time.sleep(0.4)


# ── helpers ───────────────────────────────────────────────────────────────────

def _get_state(d: Driver, pid: int, safari_pid_fixture) -> "harness.driver.WindowState":
    wid = d.find_window(pid)
    return d.get_window_state(pid, wid)


# ── tests ─────────────────────────────────────────────────────────────────────

class TestSafariButton:
    """AX-click the 'Click Me' button without stealing focus."""

    def test_click_increments_counter(self, driver, safari_pid, ux_guard):
        wid = driver.find_window(safari_pid)

        # Read full tree first to get current counter value
        full_before = driver.get_window_state(safari_pid, wid)
        count_before = _parse_clicks(full_before.tree)
        idx = full_before.find_element("Click Me")
        assert idx is not None, f"'Click Me' button not found in tree:\n{full_before.tree}"

        driver.click(safari_pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(safari_pid, wid)
        count_after = _parse_clicks(after.tree)
        print(f"\n  click counter: {count_before} → {count_after}")
        assert count_after == count_before + 1, (
            f"Counter did not increment: {count_before} → {count_after}"
        )

    def test_click_increments_twice(self, driver, safari_pid, ux_guard):
        wid = driver.find_window(safari_pid)

        # Read full tree to get current counter value
        full_before = driver.get_window_state(safari_pid, wid)
        count_before = _parse_clicks(full_before.tree)
        idx = full_before.find_element("Click Me")
        assert idx is not None

        driver.click(safari_pid, wid, element_index=idx)
        time.sleep(0.3)
        driver.click(safari_pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(safari_pid, wid)
        count_after = _parse_clicks(after.tree)
        print(f"\n  click counter: {count_before} → {count_after}")
        assert count_after == count_before + 2, (
            f"Counter should have incremented by 2: {count_before} → {count_after}"
        )


class TestSafariTextInput:
    """Type into a text field without stealing focus."""

    def test_type_text_appears_in_ax_tree(self, driver, safari_pid, ux_guard):
        wid = driver.find_window(safari_pid)

        before = driver.get_window_state(safari_pid, wid)
        idx = before.find_text_field(skip_url_bar=True)
        assert idx is not None, f"Text field not found in:\n{before.tree}"

        # Focus the field via AX click
        driver.click(safari_pid, wid, element_index=idx)
        time.sleep(0.3)

        # Clear any prior content
        driver.hotkey(safari_pid, ["cmd", "a"])
        time.sleep(0.1)

        driver.type_text_chars(safari_pid, "hello safari")
        time.sleep(0.6)

        after = driver.get_window_state(safari_pid, wid)
        assert after.has_text("hello safari"), (
            f"Typed text not visible in AX tree:\n{after.tree}"
        )

    def test_type_text_with_type_text_tool(self, driver, safari_pid, ux_guard):
        """Use type_text (not type_text_chars) and verify result."""
        wid = driver.find_window(safari_pid)

        before = driver.get_window_state(safari_pid, wid)
        idx = before.find_text_field(skip_url_bar=True)
        assert idx is not None

        driver.click(safari_pid, wid, element_index=idx)
        time.sleep(0.3)

        driver.hotkey(safari_pid, ["cmd", "a"])
        time.sleep(0.1)

        driver.type_text(safari_pid, "type_text test")
        time.sleep(0.6)

        after = driver.get_window_state(safari_pid, wid)
        assert after.has_text("type_text test"), (
            f"type_text result not in AX tree:\n{after.tree}"
        )


class TestSafariCheckbox:
    """Toggle the checkbox."""

    def test_checkbox_toggle(self, driver, safari_pid, ux_guard):
        wid = driver.find_window(safari_pid)

        before = driver.get_window_state(safari_pid, wid, query="checkbox")
        idx = Tree.find(before.tree, role="AXCheckBox")
        if idx is None:
            # Fallback: find by label text
            idx = before.find_element("unchecked")
        assert idx is not None, f"Checkbox not found:\n{before.tree}"

        driver.click(safari_pid, wid, element_index=idx)
        time.sleep(0.4)

        after = driver.get_window_state(safari_pid, wid)
        assert after.has_text("checked"), (
            f"Checkbox did not toggle to checked:\n{after.tree}"
        )


class TestSafariSelect:
    """Change the dropdown selection."""

    def test_select_option_b(self, driver, safari_pid, ux_guard):
        wid = driver.find_window(safari_pid)

        before = driver.get_window_state(safari_pid, wid, query="Option")
        idx = Tree.find(before.tree, role="AXPopUpButton")
        if idx is None:
            idx = Tree.find(before.tree, role="AXComboBox")
        if idx is None:
            idx = before.find_element("Option A")
        assert idx is not None, f"Select/dropdown not found:\n{before.tree}"

        driver.click(safari_pid, wid, element_index=idx)
        time.sleep(0.5)

        # Option B in the dropdown menu
        state2 = driver.get_window_state(safari_pid, wid)
        opt_b = Tree.find(state2.tree, label="Option B")
        if opt_b is not None:
            driver.click(safari_pid, wid, element_index=opt_b)
        else:
            # Fallback: set value directly
            driver.call_tool("set_value", {
                "pid": safari_pid, "window_id": wid,
                "element_index": idx, "value": "b",
            })
        time.sleep(0.4)

        after = driver.get_window_state(safari_pid, wid)
        assert after.has_text("Option B") or after.has_text("selected: Option B"), (
            f"Dropdown did not select Option B:\n{after.tree}"
        )


class TestSafariTextarea:
    """Type into the textarea.

    Safari's WKWebView AXTextArea does not reliably set keyboard focus via
    AXPress in background mode. We use two strategies in sequence:
    1. Click the text INPUT first to warm up Safari's web content focus engine.
    2. Then click the textarea, wait 0.8s, type.
    """

    def test_textarea_input(self, driver, safari_pid, ux_guard):
        wid = driver.find_window(safari_pid)

        state = driver.get_window_state(safari_pid, wid)

        # Step 1: click the text input first to put focus inside Safari's web content
        tf_idx = state.find_text_field(skip_url_bar=True)
        if tf_idx is not None:
            driver.click(safari_pid, wid, element_index=tf_idx)
            time.sleep(0.3)

        # Step 2: click the textarea
        ta_idx = Tree.find(state.tree, role="AXTextArea")
        assert ta_idx is not None, f"Textarea not found:\n{state.tree[:500]}"
        driver.click(safari_pid, wid, element_index=ta_idx)
        time.sleep(0.8)  # longer settle for WKWebView textarea focus

        # Step 3: type — keystrokes go to the focused element
        driver.call_tool("type_text_chars", {
            "pid": safari_pid,
            "text": "multi line",
            "delay_ms": 50,
        })
        time.sleep(0.6)

        after = driver.get_window_state(safari_pid, wid)
        val = Tree.ax_value(after.tree, ta_idx)
        print(f"\n  textarea AX value: {val!r}")
        assert after.has_text("multi line"), (
            f"Textarea text not in AX tree; val={val!r};\n{after.tree[:800]}"
        )


class TestSafariCanvas:
    """Send a pixel click to the canvas area without stealing focus.

    Background note: synthetic CGEvent pixel clicks to backgrounded browser
    windows may not trigger DOM mousedown events (web content is only rendered
    and interactive when the window is frontmost). This test therefore only
    asserts the key UX invariant: the click is delivered without error and
    without stealing focus. A DOM update (AX tree change or screenshot diff)
    is logged as a bonus if it occurs.
    """

    def test_canvas_pixel_click_no_focus_steal(self, driver, safari_pid, ux_guard):
        wid = driver.find_window(safari_pid)

        before = driver.get_window_state(safari_pid, wid)
        print(f"\n  screenshot: {before.screenshot_width}x{before.screenshot_height}")

        from harness.cv import decode, diff_ratio
        before_img = decode(before.screenshot_b64)

        # Canvas is the last section on the page. With ~80px browser chrome
        # in a 906px window, the canvas element center is near y=88% of height.
        x = before.screenshot_width // 2
        y = int(before.screenshot_height * 0.88)
        print(f"  pixel click at ({x}, {y})")

        result = driver.click(safari_pid, wid, x=x, y=y)
        time.sleep(0.5)

        after = driver.get_window_state(safari_pid, wid)

        # Bonus: check if DOM or screenshot updated (non-deterministic in background)
        canvas_clicked = "canvas: clicked" in after.tree
        after_img = decode(after.screenshot_b64)
        ratio = diff_ratio(before_img, after_img)
        print(f"  canvas JS fired: {canvas_clicked}, diff_ratio: {ratio:.4f}")

        # Primary assertion: click succeeded (no exception was raised)
        # UX assertion: via ux_guard fixture teardown (no focus steal)
        assert result is not None, "click tool returned None"


class TestSafariPressKey:
    """Press keys without stealing focus."""

    def test_tab_key(self, driver, safari_pid, ux_guard):
        driver.press_key(safari_pid, "tab")
        time.sleep(0.3)
        # No assertion on state — just verifying no exception + no UX violation

    def test_escape_key(self, driver, safari_pid, ux_guard):
        driver.press_key(safari_pid, "escape")
        time.sleep(0.2)
