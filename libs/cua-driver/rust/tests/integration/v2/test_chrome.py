"""Chrome background automation tests — v2 harness.

Mirrors test_safari.py but targets Google Chrome. The same test_page.html
is loaded via the local HTTP server. Chrome uses CGEvent/SkyLight for
background delivery which is a different code path from Safari's AX.

Run: python3 -m pytest test_chrome.py -v
"""

from __future__ import annotations

import re
import subprocess
import sys
import time
import os

import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)
from harness.driver import Driver
from harness import tree as Tree


def _parse_clicks(tree_text: str) -> int:
    m = re.search(r'clicks:\s*(\d+)', tree_text)
    return int(m.group(1)) if m else 0

CHROME_BUNDLE = "com.google.Chrome"
FOCUS_MONITOR_BUNDLE = "com.trycua.FocusMonitorApp"


# ── module-level Chrome setup ─────────────────────────────────────────────────

CHROME_APP = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"


@pytest.fixture(scope="module")
def chrome_pid(binary, html_server, focus_monitor):
    """Launch Chrome with --force-renderer-accessibility to enable AX for web content.

    Chrome only exposes web content in the AX tree when launched with this flag,
    or when an AT (Assistive Technology) has connected. Using the flag is the most
    reliable approach for integration testing.
    """
    subprocess.run(["pkill", "-x", "Google Chrome"], check=False)
    time.sleep(0.5)

    if not os.path.exists(CHROME_APP):
        pytest.skip("Google Chrome not installed at default path")

    page_url = f"{html_server}/test_page.html"
    proc = subprocess.Popen(
        [CHROME_APP, "--force-renderer-accessibility", page_url],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(5.0)  # allow full page load + AX initialization

    with Driver(binary) as d:
        app = d.find_app(CHROME_BUNDLE)
        if app is None:
            proc.terminate()
            pytest.skip("Google Chrome did not appear in list_apps")
        pid = app["pid"]

    print(f"\n  Chrome pid: {pid} (with renderer accessibility)")

    # Hand focus to FocusMonitorApp by pid to avoid activating stale instances
    _, fm_pid = focus_monitor
    subprocess.run(
        ["osascript", "-e",
         f'tell application "System Events" to set frontmost of (first process whose unix id is {fm_pid}) to true'],
        check=False,
    )
    time.sleep(0.5)

    yield pid

    proc.terminate()
    subprocess.run(["pkill", "-x", "Google Chrome"], check=False)


@pytest.fixture(autouse=True)
def _reactivate_focus(focus_monitor):
    _, pid = focus_monitor
    subprocess.run(
        ["osascript", "-e",
         f'tell application "System Events" to set frontmost of (first process whose unix id is {pid}) to true'],
        check=False,
    )
    time.sleep(0.4)


# ── tests ─────────────────────────────────────────────────────────────────────

class TestChromeButton:
    def test_click_increments_counter(self, driver, chrome_pid, ux_guard):
        wid = driver.find_window(chrome_pid)

        full = driver.get_window_state(chrome_pid, wid)
        idx = full.find_element("Click Me")
        assert idx is not None, f"'Click Me' not found in tree (first 800 chars):\n{full.tree[:800]}"
        count_before = _parse_clicks(full.tree)

        driver.click(chrome_pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(chrome_pid, wid)
        count_after = _parse_clicks(after.tree)
        print(f"\n  click counter: {count_before} → {count_after}")
        assert count_after == count_before + 1, f"Counter did not increment: {count_before} → {count_after}"

    def test_click_increments_twice(self, driver, chrome_pid, ux_guard):
        wid = driver.find_window(chrome_pid)
        full = driver.get_window_state(chrome_pid, wid)
        idx = full.find_element("Click Me")
        assert idx is not None
        count_before = _parse_clicks(full.tree)

        driver.click(chrome_pid, wid, element_index=idx)
        time.sleep(0.3)
        driver.click(chrome_pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(chrome_pid, wid)
        count_after = _parse_clicks(after.tree)
        print(f"\n  click counter: {count_before} → {count_after}")
        assert count_after == count_before + 2, f"Counter should have incremented by 2: {count_before} → {count_after}"


class TestChromeTextInput:
    def test_type_text_appears(self, driver, chrome_pid, ux_guard):
        wid = driver.find_window(chrome_pid)
        before = driver.get_window_state(chrome_pid, wid)
        idx = before.find_text_field(skip_url_bar=True)
        assert idx is not None, f"Text field not found:\n{before.tree}"

        driver.click(chrome_pid, wid, element_index=idx)
        time.sleep(0.3)
        driver.hotkey(chrome_pid, ["cmd", "a"])
        time.sleep(0.1)
        driver.type_text_chars(chrome_pid, "hello chrome")
        time.sleep(0.6)

        after = driver.get_window_state(chrome_pid, wid)
        assert after.has_text("hello chrome"), f"Text not found:\n{after.tree}"


class TestChromeCanvas:
    def test_canvas_pixel_click_no_focus_steal(self, driver, chrome_pid, ux_guard):
        """Send a pixel click to the canvas area without stealing focus.

        Canvas DOM events may not fire in background; we verify the click is
        delivered without error and without UX violations via ux_guard.
        """
        wid = driver.find_window(chrome_pid)
        mid = driver.get_window_state(chrome_pid, wid)
        assert mid.screenshot_b64, "No screenshot"

        from harness.cv import decode, diff_ratio
        before_img = decode(mid.screenshot_b64)

        x = mid.screenshot_width // 2
        y = int(mid.screenshot_height * 0.88)
        print(f"\n  chrome canvas click at ({x}, {y})")

        result = driver.click(chrome_pid, wid, x=x, y=y)
        time.sleep(0.5)

        after = driver.get_window_state(chrome_pid, wid)
        after_img = decode(after.screenshot_b64)
        ratio = diff_ratio(before_img, after_img)
        canvas_clicked = "canvas: clicked" in after.tree
        print(f"  chrome canvas diff_ratio: {ratio:.4f}, JS fired: {canvas_clicked}")

        assert result is not None, "click returned None"
        # UX assertions via ux_guard teardown


class TestChromePressKey:
    def test_tab_key(self, driver, chrome_pid, ux_guard):
        driver.press_key(chrome_pid, "tab")
        time.sleep(0.3)

    def test_escape_key(self, driver, chrome_pid, ux_guard):
        driver.press_key(chrome_pid, "escape")
        time.sleep(0.2)
