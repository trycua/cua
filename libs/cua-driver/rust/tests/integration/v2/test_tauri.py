"""Tauri desktop-test-app background automation tests — v2 harness.

The Tauri test app (v0.2.2) renders a 4-panel WKWebView UI and exposes
an HTTP API on port 6769 that logs all mouse/keyboard/clipboard events.

Primary assertion: AX tree state changes.
Cross-check: HTTP event log.

Run: python3 -m pytest test_tauri.py -v
"""

from __future__ import annotations

import subprocess
import sys
import time
import os

import pytest

try:
    import requests as _requests
    _REQUESTS_AVAILABLE = True
except ImportError:
    _REQUESTS_AVAILABLE = False

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)
from harness.driver import Driver
from harness import tree as Tree

_TAURI_API = "http://localhost:6769"


def _http_events(base_url: str = _TAURI_API) -> list[dict]:
    if not _REQUESTS_AVAILABLE:
        return []
    try:
        resp = _requests.get(f"{base_url}/events", timeout=2)
        return resp.json() if resp.ok else []
    except Exception:
        return []


def _http_reset(base_url: str = _TAURI_API) -> None:
    if not _REQUESTS_AVAILABLE:
        return
    try:
        _requests.post(f"{base_url}/reset", timeout=2)
    except Exception:
        pass


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
        _, pid, base_url = tauri_app
        if pid == 0:
            pytest.skip("Tauri app pid not found")

        _http_reset(base_url)
        wid = driver.find_window(pid)

        before = driver.get_window_state(pid, wid, query="button")
        idx = before.find_element("click")
        if idx is None:
            idx = Tree.find(before.tree, role="AXButton")
        assert idx is not None, f"Button not found:\n{before.tree}"

        driver.click(pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(pid, wid)
        # Verify some state change happened (counter or AX value)
        changed = after.tree != before.tree
        print(f"\n  tauri tree changed: {changed}")

        # Cross-check: HTTP events should contain a mouse event
        events = _http_events(base_url)
        print(f"  tauri events: {len(events)}")
        # Either AX tree changed or HTTP logged a mouse event
        assert changed or any(
            e.get("type") in ("mouse_down", "click", "mousedown") for e in events
        ), f"No observable change after button click:\n{after.tree}"


class TestTauriTextInput:
    def test_type_text(self, driver, tauri_app, ux_guard):
        _, pid, base_url = tauri_app
        if pid == 0:
            pytest.skip("Tauri app pid not found")

        _http_reset(base_url)
        wid = driver.find_window(pid)

        before = driver.get_window_state(pid, wid)
        idx = before.find_text_field(skip_url_bar=False)
        if idx is None:
            idx = Tree.find(before.tree, role="AXTextField")
        if idx is None:
            idx = Tree.find(before.tree, role="AXTextArea")
        assert idx is not None, f"Text input not found:\n{before.tree}"

        driver.click(pid, wid, element_index=idx)
        time.sleep(0.3)
        driver.hotkey(pid, ["cmd", "a"])
        time.sleep(0.1)
        driver.type_text_chars(pid, "tauri test")
        time.sleep(0.6)

        after = driver.get_window_state(pid, wid)
        text_in_tree = after.has_text("tauri test")
        print(f"\n  text in AX tree: {text_in_tree}")

        events = _http_events(base_url)
        key_events = [e for e in events if e.get("type") in ("key_down", "keydown", "input")]
        print(f"  key events: {len(key_events)}")

        assert text_in_tree or len(key_events) >= len("tauri test"), (
            f"Text not found in AX tree and insufficient key events:\n{after.tree}"
        )


class TestTauriNoFocusSteal:
    def test_multiple_actions_no_focus_steal(self, driver, tauri_app, ux_guard):
        _, pid, base_url = tauri_app
        if pid == 0:
            pytest.skip("Tauri app pid not found")

        wid = driver.find_window(pid)

        # Take 3 actions in sequence — UX guard checks no focus steal
        before = driver.get_window_state(pid, wid)
        idx = Tree.find(before.tree, role="AXButton")
        if idx is not None:
            driver.click(pid, wid, element_index=idx)
            time.sleep(0.3)

        driver.press_key(pid, "tab")
        time.sleep(0.2)
        driver.press_key(pid, "tab")
        time.sleep(0.2)
        # ux_guard teardown will assert_clean()
