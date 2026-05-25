"""Integration tests: WKWebView/Tauri AX fallback (page tool).

Tests AX-based get_text and query_dom for Tauri/WKWebView apps where the
WebKit remote inspector is blocked, and verifies that:
  - execute_javascript returns a clear error pointing to AX alternatives
  - get_text returns content via the AX tree
  - query_dom returns elements via the AX tree
  - Chrome is NOT misdetected as a WKWebView app

Requires Conductor.app (/Applications/Conductor.app) — a Tauri app.
Tests that need Conductor skip when it is not installed.

Run:
    scripts/test.sh test_webkit_js
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path

CONDUCTOR_BUNDLE = "com.conductor.app"
CONDUCTOR_PATH   = "/Applications/Conductor.app"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _tool_text(result: dict) -> str:
    for item in result.get("content", []):
        if item.get("type") == "text":
            return item.get("text", "")
    return ""


def _conductor_installed() -> bool:
    return os.path.isdir(CONDUCTOR_PATH)


def _kill_conductor() -> None:
    subprocess.run(
        ["osascript", "-e", 'tell application "Conductor" to quit'],
        check=False, timeout=5,
    )
    time.sleep(1)


def _find_pid(text: str) -> int | None:
    m = re.search(r'pid[=:\s]+(\d+)', text, re.IGNORECASE)
    return int(m.group(1)) if m else None


def _main_window(client: DriverClient, pid: int) -> int | None:
    """Return the on-screen Conductor window_id, or None if not found."""
    text = _tool_text(client.call_tool("list_windows", {"pid": pid}))
    for line in text.splitlines():
        if "on_screen=True" in line or "is_on_screen=True" in line:
            m = re.search(r'\[window_id:\s*(\d+)\]', line)
            if m:
                return int(m.group(1))
    # Fall back to any titled window
    m = re.search(r'"[^"]+"\s+\[window_id:\s*(\d+)\]', text)
    if m:
        return int(m.group(1))
    m = re.search(r'\[window_id:\s*(\d+)\]', text)
    return int(m.group(1)) if m else None


# ---------------------------------------------------------------------------
# Conductor / Tauri AX fallback tests
# ---------------------------------------------------------------------------

@unittest.skipUnless(_conductor_installed(), "Conductor not installed")
class WebKitJSTests(unittest.TestCase):
    """AX-based page primitives work end-to-end against a live Conductor window."""

    _pid: int
    _window_id: int

    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = default_binary_path()
        _kill_conductor()

        with DriverClient(cls.binary) as c:
            result = c.call_tool("launch_app", {"bundle_id": CONDUCTOR_BUNDLE})
            text = _tool_text(result)
            cls._pid = _find_pid(text) or 0

        if not cls._pid:
            raise RuntimeError("Could not determine Conductor pid from launch_app result")

        deadline = time.monotonic() + 10.0
        cls._window_id = None
        while time.monotonic() < deadline:
            with DriverClient(cls.binary) as c:
                wid = _main_window(c, cls._pid)
                if wid:
                    cls._window_id = wid
                    break
            time.sleep(0.5)

        if not cls._window_id:
            raise RuntimeError(f"No window found for Conductor pid {cls._pid}")

        print(f"\n  Conductor pid={cls._pid} window_id={cls._window_id}")

    @classmethod
    def tearDownClass(cls) -> None:
        _kill_conductor()

    # -----------------------------------------------------------------------
    # execute_javascript — must return WKWebView error, not crash
    # -----------------------------------------------------------------------

    def test_13_execute_javascript_returns_clear_error_for_wkwebview(self) -> None:
        """execute_javascript on a WKWebView app returns a clear error."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._pid,
                "window_id": self._window_id,
                "action": "execute_javascript",
                "javascript": "document.title",
            })
        self.assertTrue(result.get("isError"), "Expected isError=True for WKWebView JS")
        text = _tool_text(result)
        self.assertIn("WKWebView", text)
        self.assertIn("get_text", text)

    # -----------------------------------------------------------------------
    # get_text — must return content via AX tree
    # -----------------------------------------------------------------------

    def test_11_ax_get_text_works_without_inspector(self) -> None:
        """get_text returns page text via AX tree (no inspector needed)."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._pid,
                "window_id": self._window_id,
                "action": "get_text",
            })
        self.assertFalse(result.get("isError"), _tool_text(result))
        text = _tool_text(result)
        self.assertTrue(len(text.strip()) > 0, "get_text returned empty content")

    # -----------------------------------------------------------------------
    # query_dom — must return AX elements via AX tree
    # -----------------------------------------------------------------------

    def test_12_ax_query_dom_buttons(self) -> None:
        """query_dom for button elements returns AX tree results."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._pid,
                "window_id": self._window_id,
                "action": "query_dom",
                "css_selector": "button",
            })
        text = _tool_text(result)
        self.assertNotIn("Traceback", text)
        # Either found buttons (role key) or returned empty — both are valid
        if not result.get("isError"):
            self.assertTrue(
                "AXButton" in text or "role" in text or "[]" in text,
                f"Unexpected query_dom output: {text[:200]}"
            )

    # -----------------------------------------------------------------------
    # Chrome must NOT be misdetected as WKWebView
    # -----------------------------------------------------------------------

    def test_chrome_not_detected_as_wkwebview(self) -> None:
        """Chrome execute_javascript must not return the WKWebView error."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("list_apps")
            text = _tool_text(result)

        chrome_pid = None
        for line in text.splitlines():
            if "com.google.Chrome" in line:
                m = re.search(r'pid[=:\s]+(\d+)', line, re.IGNORECASE)
                if m:
                    chrome_pid = int(m.group(1))
                    break

        if chrome_pid is None:
            self.skipTest("Chrome not running")

        with DriverClient(self.binary) as c:
            windows_text = _tool_text(c.call_tool("list_windows", {"pid": chrome_pid}))

        m = re.search(r'\[window_id:\s*(\d+)\]', windows_text)
        if not m:
            self.skipTest("No Chrome window found")
        window_id = int(m.group(1))

        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": chrome_pid,
                "window_id": window_id,
                "action": "execute_javascript",
                "javascript": "1+1",
            })
        text = _tool_text(result)
        self.assertNotIn(
            "WKWebView", text,
            f"Chrome was incorrectly detected as WKWebView app: {text[:200]}"
        )


if __name__ == "__main__":
    unittest.main()
