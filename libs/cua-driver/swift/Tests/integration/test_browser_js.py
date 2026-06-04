"""Integration tests: browser JS primitives (page tool + get_window_state javascript param).

Tests the three new browser primitives:
  - page(action=get_text)          — document.body.innerText
  - page(action=query_dom)         — querySelectorAll as JSON
  - page(action=execute_javascript)— raw JS with return value
  - get_window_state(javascript=…) — JS result alongside AX snapshot

Setup:
  Requires Chrome to be running with "Allow JavaScript from Apple Events"
  enabled. The test enables it automatically using the Preferences JSON path
  documented in WEB_APPS.md (quits Chrome, writes pref, relaunches).

Run:
    scripts/test.sh test_browser_js
"""

from __future__ import annotations

import glob
import json
import os
import re
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path

CHROME_BUNDLE = "com.google.Chrome"
# Simple, stable public page with predictable DOM: one <h1>, one <a>.
TEST_URL = "https://example.com"
# Expected content from example.com.
EXPECTED_H1 = "Example Domain"
EXPECTED_LINK_TEXT = "Learn more"
EXPECTED_HREF_FRAGMENT = "iana.org/domains/example"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _tool_text(result: dict) -> str:
    """Extract the text string from a tool call result."""
    for item in result.get("content", []):
        if item.get("type") == "text":
            return item.get("text", "")
    return ""


def _enable_chrome_apple_events() -> None:
    """Quit Chrome, write allow_javascript_apple_events to all profiles, relaunch."""
    subprocess.run(
        ["osascript", "-e", 'quit app "Google Chrome"'],
        check=False, timeout=10,
    )
    time.sleep(1.5)

    for prefs_path in glob.glob(
        os.path.expanduser(
            "~/Library/Application Support/Google/Chrome/*/Preferences"
        )
    ):
        profile = prefs_path.split("/")[-2]
        if "System" in profile or "Guest" in profile:
            continue
        try:
            with open(prefs_path) as f:
                data = json.load(f)
            data.setdefault("browser", {})["allow_javascript_apple_events"] = True
            data.setdefault("account_values", {}).setdefault("browser", {})[
                "allow_javascript_apple_events"
            ] = True
            with open(prefs_path, "w") as f:
                json.dump(data, f)
        except Exception as e:
            print(f"  skipped {profile}: {e}")

    subprocess.run(["open", "-a", "Google Chrome"], check=True)
    # 0.8s: test before sync fires so local True is still in effect.
    time.sleep(0.8)


def _find_chrome(client: DriverClient) -> int:
    """Return Chrome's pid from list_apps text output."""
    text = _tool_text(client.call_tool("list_apps"))
    m = re.search(r"Google Chrome \(pid (\d+)\)", text)
    if not m:
        raise RuntimeError("Chrome not found in list_apps output")
    return int(m.group(1))


def _main_window(client: DriverClient, pid: int) -> int:
    """Return window_id of the Chrome window that contains example.com."""
    text = _tool_text(client.call_tool("list_windows", {"pid": pid}))
    # Lines look like: - Google Chrome (pid N) "Title" [window_id: NNNN]
    # Prefer a window whose title contains "Example Domain".
    for line in text.splitlines():
        if "Example Domain" in line or "example.com" in line.lower():
            m = re.search(r'\[window_id:\s*(\d+)\]', line)
            if m:
                return int(m.group(1))
    # Fall back to first titled window.
    titled = re.findall(r'"[^"]+"\s+\[window_id:\s*(\d+)\]', text)
    if titled:
        return int(titled[0])
    m = re.search(r'\[window_id:\s*(\d+)\]', text)
    if m:
        return int(m.group(1))
    raise RuntimeError(f"No window found for Chrome pid {pid}")


# ---------------------------------------------------------------------------
# Test class
# ---------------------------------------------------------------------------

class BrowserJSTests(unittest.TestCase):
    """Browser JS primitives work end-to-end against a live Chrome window."""

    _chrome_pid: int
    _window_id: int

    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = default_binary_path()

        _enable_chrome_apple_events()

        # launch_app opens the URL in a new window WITHOUT stealing focus —
        # the driver's FocusRestoreGuard catches Chrome's activate call.
        with DriverClient(cls.binary) as c:
            result = c.call_tool("launch_app", {
                "bundle_id": CHROME_BUNDLE,
                "urls": [TEST_URL],
            })
            text = _tool_text(result)
            # Extract pid from launch_app response.
            m = re.search(r'pid[=:\s]+(\d+)', text, re.IGNORECASE)
            cls._chrome_pid = int(m.group(1)) if m else _find_chrome(c)

        # Wait for Chrome to load the page and update the window title.
        # Retry up to 8 s — network load time varies.
        deadline = time.monotonic() + 8.0
        cls._window_id = None
        while time.monotonic() < deadline:
            with DriverClient(cls.binary) as c:
                try:
                    cls._window_id = _main_window(c, cls._chrome_pid)
                    break
                except RuntimeError:
                    pass
            time.sleep(0.5)

        if cls._window_id is None:
            raise RuntimeError(
                f"example.com window never appeared for Chrome pid {cls._chrome_pid}"
            )

        print(f"\n  Chrome pid={cls._chrome_pid} window_id={cls._window_id}")

    # -----------------------------------------------------------------------
    # page action=execute_javascript
    # -----------------------------------------------------------------------

    def test_01_execute_javascript_arithmetic(self) -> None:
        """execute_javascript returns the JS evaluation result."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "execute_javascript",
                "javascript": "1 + 1",
            })
        self.assertFalse(result.get("isError"))
        self.assertIn("2", _tool_text(result))

    def test_02_execute_javascript_dom_read(self) -> None:
        """execute_javascript can read DOM content."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "execute_javascript",
                "javascript": "document.querySelector('h1').innerText",
            })
        self.assertFalse(result.get("isError"))
        self.assertIn(EXPECTED_H1, _tool_text(result))

    def test_03_execute_javascript_iife(self) -> None:
        """execute_javascript handles IIFE with try-catch."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "execute_javascript",
                "javascript": (
                    "(() => { try { return document.title; } "
                    "catch(e) { return 'error: ' + e; } })()"
                ),
            })
        self.assertFalse(result.get("isError"))

    # -----------------------------------------------------------------------
    # page action=get_text
    # -----------------------------------------------------------------------

    def test_04_get_text_returns_body_text(self) -> None:
        """get_text returns document.body.innerText containing the H1."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "get_text",
            })
        self.assertFalse(result.get("isError"))
        self.assertIn(EXPECTED_H1, _tool_text(result))

    def test_05_get_text_includes_link_text(self) -> None:
        """get_text includes anchor text from the page."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "get_text",
            })
        text = _tool_text(result)
        self.assertIn(EXPECTED_LINK_TEXT, text,
                      f"Expected link text in body text, got: {text[:200]!r}")

    # -----------------------------------------------------------------------
    # page action=query_dom
    # -----------------------------------------------------------------------

    def test_06_query_dom_returns_json_array(self) -> None:
        """query_dom returns a JSON array of matching elements with hrefs."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "query_dom",
                "css_selector": "a[href]",
                "attributes": ["href"],
            })
        self.assertFalse(result.get("isError"))
        text = _tool_text(result)
        self.assertIn("```json", text)
        json_block = text.split("```json")[-1].split("```")[0].strip()
        items = json.loads(json_block)
        self.assertIsInstance(items, list)
        self.assertGreaterEqual(len(items), 1)
        hrefs = {item.get("href") or "" for item in items}
        self.assertTrue(
            any(EXPECTED_HREF_FRAGMENT in h for h in hrefs),
            f"Expected href containing '{EXPECTED_HREF_FRAGMENT}', got: {hrefs}",
        )

    def test_07_query_dom_h1_no_attributes(self) -> None:
        """query_dom without attributes returns tag and text."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "query_dom",
                "css_selector": "h1",
            })
        self.assertFalse(result.get("isError"))
        text = _tool_text(result)
        json_block = text.split("```json")[-1].split("```")[0].strip()
        items = json.loads(json_block)
        self.assertEqual(len(items), 1)
        self.assertEqual(items[0]["tag"], "h1")
        self.assertIn(EXPECTED_H1, items[0]["text"])

    # -----------------------------------------------------------------------
    # get_window_state javascript param
    # -----------------------------------------------------------------------

    def test_08_get_window_state_javascript_co_located(self) -> None:
        """get_window_state with javascript= returns JS result alongside AX tree."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("get_window_state", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "javascript": "document.title",
            })
        self.assertFalse(result.get("isError"))
        text = _tool_text(result)
        self.assertIn("## JavaScript result", text)
        # AX tree content is also present.
        self.assertRegex(text, r"elements.*turn", re.IGNORECASE)

    def test_09_get_window_state_javascript_error_is_inline(self) -> None:
        """JS runtime errors in get_window_state appear inline, tool does not isError."""
        # Chrome returns "missing value" for JS that throws — osascript exits 0.
        # The result section appears but may contain "missing value" or an error msg.
        with DriverClient(self.binary) as c:
            result = c.call_tool("get_window_state", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "javascript": "undefined_var_that_does_not_exist",
            })
        # isError must be False — JS failures must not kill the whole tool call.
        self.assertFalse(result.get("isError"))
        text = _tool_text(result)
        # The JS result section must always appear.
        self.assertIn("## JavaScript result", text)

    # -----------------------------------------------------------------------
    # Error cases
    # -----------------------------------------------------------------------

    def test_10_page_execute_missing_javascript_field(self) -> None:
        """page execute_javascript returns isError when javascript field is absent."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "execute_javascript",
            })
        self.assertTrue(result.get("isError"))

    def test_11_page_query_dom_missing_selector(self) -> None:
        """page query_dom returns isError when css_selector is absent."""
        with DriverClient(self.binary) as c:
            result = c.call_tool("page", {
                "pid": self._chrome_pid,
                "window_id": self._window_id,
                "action": "query_dom",
            })
        self.assertTrue(result.get("isError"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
