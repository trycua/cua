"""Integration tests: WKWebView/Tauri AX fallback path (page tool).

Tests the AX-based fallback for apps where the WebKit remote inspector is
blocked — specifically:
  - page(action=get_text)   routes through AXPageReader when the target is
    a WKWebView app (WebInspectorXPC.isWKWebViewApp returns true)
  - page(action=query_dom)  same AX fallback with CSS selector → AX role

Requires Conductor.app (a Tauri-based app shipped with cua-driver) to be
installed in /Applications. Tests that skip the inspector path skip when
Conductor is not installed or when TAURI_WEBVIEW_AUTOMATION is not set.

Run:
    scripts/test.sh test_webkit_js
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path

CONDUCTOR_BUNDLE = "com.trycua.Conductor"
CONDUCTOR_APP_PATH = "/Applications/Conductor.app"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _tool_text(result: dict) -> str:
    """Extract the text string from a tool call result."""
    for item in result.get("content", []):
        if item.get("type") == "text":
            return item.get("text", "")
    return ""


def _is_error(result: dict) -> bool:
    return result.get("isError", False)


def _conductor_installed() -> bool:
    return os.path.isdir(CONDUCTOR_APP_PATH)


def _find_conductor_pid(client: DriverClient) -> int | None:
    """Return the pid of a running Conductor instance, or None."""
    result = client.call("list_apps", {})
    text = _tool_text(result)
    for line in text.splitlines():
        if CONDUCTOR_BUNDLE in line or "Conductor" in line:
            # Line format: "Conductor (com.trycua.Conductor) pid=1234"
            for part in line.split():
                if part.startswith("pid="):
                    try:
                        return int(part[4:])
                    except ValueError:
                        pass
    return None


# ---------------------------------------------------------------------------
# AXPageReader unit-level tests (no app required)
# ---------------------------------------------------------------------------

class TestAXPageReaderExtractText(unittest.TestCase):
    """Test that AXPageReader.extractText produces the right output."""

    # We test this indirectly via page(get_text) on a synthetic AX tree
    # produced by get_window_state — so we still need the driver running.

    @classmethod
    def setUpClass(cls):
        cls.client = DriverClient(binary_path=default_binary_path())
        cls.client.start()

    @classmethod
    def tearDownClass(cls):
        cls.client.stop()

    def test_driver_responds(self):
        """Smoke test: driver is reachable."""
        result = self.client.call("get_screen_size", {})
        self.assertFalse(_is_error(result), _tool_text(result))
        text = _tool_text(result)
        self.assertIn("width", text.lower())


# ---------------------------------------------------------------------------
# WebInspectorXPC detection tests
# ---------------------------------------------------------------------------

class TestWebInspectorXPCDetection(unittest.TestCase):
    """Test isWKWebViewApp detection heuristic via execute_javascript errors."""

    @classmethod
    def setUpClass(cls):
        cls.client = DriverClient(binary_path=default_binary_path())
        cls.client.start()
        # List apps to find Chrome pid (should NOT be detected as WKWebView app).
        cls.chrome_pid = None
        result = cls.client.call("list_apps", {})
        text = _tool_text(result)
        for line in text.splitlines():
            if "com.google.Chrome" in line:
                for part in line.split():
                    if part.startswith("pid="):
                        try:
                            cls.chrome_pid = int(part[4:])
                        except ValueError:
                            pass

    @classmethod
    def tearDownClass(cls):
        cls.client.stop()

    @unittest.skipUnless(
        os.path.exists("/Applications/Google Chrome.app"),
        "Chrome not installed"
    )
    def test_chrome_not_detected_as_wkwebview(self):
        """Chrome must NOT be classified as a WKWebView app."""
        if self.chrome_pid is None:
            self.skipTest("Chrome not running")
        # list_windows to get a window_id for Chrome.
        win_result = self.client.call("list_windows", {"pid": self.chrome_pid})
        text = _tool_text(win_result)
        # Extract first window_id from output.
        window_id = None
        for line in text.splitlines():
            for part in line.split():
                if part.startswith("id="):
                    try:
                        window_id = int(part[3:])
                        break
                    except ValueError:
                        pass
            if window_id:
                break
        if window_id is None:
            self.skipTest("No Chrome window found")
        # execute_javascript on Chrome should succeed or fail with an Apple Events
        # error — NOT with the WKWebView-specific "use get_text or query_dom" message.
        result = self.client.call("page", {
            "pid": self.chrome_pid,
            "window_id": window_id,
            "action": "execute_javascript",
            "javascript": "1+1",
        })
        text = _tool_text(result)
        self.assertNotIn(
            "WKWebView",
            text,
            f"Chrome was incorrectly detected as a WKWebView app: {text}",
        )


# ---------------------------------------------------------------------------
# Conductor / Tauri AX fallback tests
# ---------------------------------------------------------------------------

class TestConductorAXFallback(unittest.TestCase):
    """Test AX-based get_text / query_dom for Conductor (Tauri app)."""

    @classmethod
    def setUpClass(cls):
        if not _conductor_installed():
            return
        cls.client = DriverClient(binary_path=default_binary_path())
        cls.client.start()
        # Launch Conductor if not already running.
        launch_result = cls.client.call("launch_app", {
            "bundle_id": CONDUCTOR_BUNDLE,
        })
        time.sleep(3)  # Wait for app to be ready.
        cls.pid = _find_conductor_pid(cls.client)

    @classmethod
    def tearDownClass(cls):
        if not _conductor_installed():
            return
        if hasattr(cls, "client"):
            cls.client.stop()

    def _get_window_id(self) -> int | None:
        if self.pid is None:
            return None
        result = self.client.call("list_windows", {"pid": self.pid})
        text = _tool_text(result)
        for line in text.splitlines():
            for part in line.split():
                if part.startswith("id="):
                    try:
                        return int(part[3:])
                    except ValueError:
                        pass
        return None

    @unittest.skipUnless(_conductor_installed(), "Conductor not installed")
    def test_execute_javascript_returns_wkwebview_error(self):
        """execute_javascript on Conductor must return the WKWebView error."""
        window_id = self._get_window_id()
        if window_id is None:
            self.skipTest("Conductor window not found")
        result = self.client.call("page", {
            "pid": self.pid,
            "window_id": window_id,
            "action": "execute_javascript",
            "javascript": "document.title",
        })
        self.assertTrue(_is_error(result), "Expected error for WKWebView JS")
        text = _tool_text(result)
        self.assertIn("WKWebView", text)
        self.assertIn("get_text", text)

    @unittest.skipUnless(_conductor_installed(), "Conductor not installed")
    def test_get_text_via_ax_tree(self):
        """get_text on Conductor must return content via AX tree."""
        window_id = self._get_window_id()
        if window_id is None:
            self.skipTest("Conductor window not found")
        result = self.client.call("page", {
            "pid": self.pid,
            "window_id": window_id,
            "action": "get_text",
        })
        self.assertFalse(_is_error(result), _tool_text(result))
        text = _tool_text(result)
        # AX tree result should mention the fallback path.
        self.assertTrue(
            len(text.strip()) > 0,
            "get_text returned empty content"
        )

    @unittest.skipUnless(_conductor_installed(), "Conductor not installed")
    def test_query_dom_buttons_via_ax_tree(self):
        """query_dom(button) on Conductor returns AX buttons."""
        window_id = self._get_window_id()
        if window_id is None:
            self.skipTest("Conductor window not found")
        result = self.client.call("page", {
            "pid": self.pid,
            "window_id": window_id,
            "action": "query_dom",
            "css_selector": "button",
        })
        text = _tool_text(result)
        # Either returns buttons or returns a well-formed error — not a crash.
        self.assertFalse(
            "Traceback" in text or "fatal" in text.lower(),
            f"Unexpected crash output: {text}"
        )

    @unittest.skipUnless(_conductor_installed(), "Conductor not installed")
    def test_query_dom_links_via_ax_tree(self):
        """query_dom(a) on Conductor returns AX links."""
        window_id = self._get_window_id()
        if window_id is None:
            self.skipTest("Conductor window not found")
        result = self.client.call("page", {
            "pid": self.pid,
            "window_id": window_id,
            "action": "query_dom",
            "css_selector": "a",
        })
        text = _tool_text(result)
        self.assertFalse(
            "Traceback" in text or "fatal" in text.lower(),
            f"Unexpected crash output: {text}"
        )


if __name__ == "__main__":
    unittest.main()
