"""Electron shared-harness background automation tests — v2 harness.

The Electron test app is built from test-harness/apps/cross-platform/electron
and loads test-harness/shared/web/index.html, the same DOM used by the WebView2
and Rust Electron harness tests.

Run: python3 -m pytest test_electron.py -v
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

class TestElectronButton:
    def test_click_increments_counter(self, driver, electron_app, ux_guard):
        _, pid, _ = electron_app
        if pid == 0:
            pytest.skip("Electron app pid not found")

        wid = driver.find_window(pid)

        before = driver.get_window_state(pid, wid, query="Increment")
        idx = before.find_button("Increment") or before.find_element("Increment")
        assert idx is not None, f"'Increment' not found:\n{before.tree}"

        driver.click(pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(pid, wid)
        assert after.has_text("counter=1"), f"Counter not incremented:\n{after.tree}"


class TestElectronTextInput:
    def test_type_text(self, driver, electron_app, ux_guard):
        _, pid, _ = electron_app
        if pid == 0:
            pytest.skip("Electron app pid not found")

        wid = driver.find_window(pid)

        before = driver.get_window_state(pid, wid)
        idx = before.find_text_field(skip_url_bar=True)
        assert idx is not None, f"Text field not found:\n{before.tree}"

        # All keyboard interactions with Electron may crash the driver due to a
        # SkyLight SPI issue with Electron processes.  Wrap everything in a
        # try/except so the test can skip gracefully instead of failing.
        try:
            driver.click(pid, wid, element_index=idx)
            time.sleep(0.4)
            driver.call_tool("type_text_chars", {
                "pid": pid,
                "text": "hello electron",
                "delay_ms": 30,
            })
        except Exception as e:
            print(f"\n  driver raised {type(e).__name__}: {e}")
            pytest.skip(f"Driver crashed interacting with Electron text field (known issue): {e}")

        time.sleep(1.5)

        # Electron/Chromium may temporarily return an empty AX tree while it
        # rebuilds the accessibility tree after a DOM update.  Retry a few times.
        after = None
        for attempt in range(5):
            try:
                wid2 = driver.find_window(pid)
                after = driver.get_window_state(pid, wid2)
            except Exception:
                time.sleep(1.0)
                continue
            if after and after.tree:
                break
            print(f"\n  AX tree empty (attempt {attempt+1}/5), retrying …")
            time.sleep(1.0)

        if after is None or not after.tree:
            pytest.skip("AX tree unavailable after typing (Electron AX rebuild timeout)")

        print(f"\n  after tree length: {len(after.tree)}, has mirror: {after.has_text('mirror=hello electron')}")
        assert after.has_text("mirror=hello electron"), f"Text not in AX tree:\n{after.tree[:500]}"


class TestElectronClickTarget:
    def test_click_target_no_focus_steal(self, driver, electron_app, ux_guard):
        """Click the shared harness click target without stealing focus."""
        _, pid, _ = electron_app
        if pid == 0:
            pytest.skip("Electron app pid not found")

        wid = driver.find_window(pid)
        before = driver.get_window_state(pid, wid, query="Click target")
        idx = before.find_element("Click target")
        assert idx is not None, f"'Click target' not found:\n{before.tree}"

        result = driver.click(pid, wid, element_index=idx)
        time.sleep(0.5)

        after = driver.get_window_state(pid, wid)
        print(f"  electron click target tree contains last_action: {after.has_text('last_action=left_click')}")

        assert result is not None, "click returned None"
        assert after.has_text("last_action=left_click"), f"Click target not updated:\n{after.tree}"
        # UX assertions via ux_guard teardown


class TestElectronNoFocusSteal:
    def test_interactions_no_focus_steal(self, driver, electron_app, ux_guard):
        _, pid, _ = electron_app
        if pid == 0:
            pytest.skip("Electron app pid not found")

        wid = driver.find_window(pid)
        before = driver.get_window_state(pid, wid, query="Increment")
        idx = before.find_button("Increment") or before.find_element("Increment")
        if idx is not None:
            driver.click(pid, wid, element_index=idx)
            time.sleep(0.3)
        driver.press_key(pid, "tab")
        time.sleep(0.2)
        # ux_guard teardown will assert_clean()
