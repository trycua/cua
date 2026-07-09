"""Tauri shared-harness background automation tests.

The Tauri test app is built from test-harness/apps/cross-platform/tauri
and loads test-harness/shared/web/index.html, the same DOM used by the
Electron, WebView2, and WKWebView harnesses.

Run: python3 -m pytest test_tauri.py -v
"""

from __future__ import annotations

import subprocess
import sys
import time
import os

import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)


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

class TestTauriButton:
    def test_button_click(self, driver, tauri_app, ux_guard):
        _, pid, _ = tauri_app
        if pid == 0:
            pytest.skip("Tauri app pid not found")

        wid = driver.find_window(pid)

        before = driver.get_window_state(pid, wid, query="Increment")
        idx = before.find_button("Increment") or before.find_element("Increment")
        assert idx is not None, f"'Increment' not found:\n{before.tree}"

        driver.click(pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(pid, wid)
        assert after.has_text("counter=1"), f"Counter not incremented:\n{after.tree}"


class TestTauriTextInput:
    def test_type_text(self, driver, tauri_app, ux_guard):
        _, pid, _ = tauri_app
        if pid == 0:
            pytest.skip("Tauri app pid not found")

        wid = driver.find_window(pid)

        before = driver.get_window_state(pid, wid)
        idx = before.find_text_field(skip_url_bar=False)
        assert idx is not None, f"Text input not found:\n{before.tree}"

        driver.click(pid, wid, element_index=idx)
        time.sleep(0.3)
        driver.hotkey(pid, ["cmd", "a"])
        time.sleep(0.1)
        driver.type_text_chars(pid, "tauri test")
        time.sleep(0.6)

        after = driver.get_window_state(pid, wid)
        assert after.has_text("mirror=tauri test"), f"Text not mirrored in AX tree:\n{after.tree}"


class TestTauriNoFocusSteal:
    def test_multiple_actions_no_focus_steal(self, driver, tauri_app, ux_guard):
        _, pid, _ = tauri_app
        if pid == 0:
            pytest.skip("Tauri app pid not found")

        wid = driver.find_window(pid)

        # Take 3 actions in sequence — UX guard checks no focus steal
        before = driver.get_window_state(pid, wid, query="Increment")
        idx = before.find_button("Increment") or before.find_element("Increment")
        if idx is not None:
            driver.click(pid, wid, element_index=idx)
            time.sleep(0.3)

        driver.press_key(pid, "tab")
        time.sleep(0.2)
        driver.press_key(pid, "tab")
        time.sleep(0.2)
        # ux_guard teardown will assert_clean()
