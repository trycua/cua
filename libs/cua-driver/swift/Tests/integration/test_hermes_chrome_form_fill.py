"""Integration test: hermes CLI drives Chrome form fields in the background.

Goal
----
One test per HTML input type. Each test asks hermes to interact with exactly
ONE element in a Chrome window that is NOT frontmost — verifying that:

  1. cua-driver delivers events to Chromium inputs without stealing focus.
     The unified ``type_text`` tool automatically falls back to CGEvent
     synthesis when its AX attribute-write path is rejected by Chrome's
     web-content inputs — callers (hermes) don't need to know which path ran.

  2. ``launch_app`` (not ``open -a``) is used to keep Chrome hidden and
     avoid the self-activation race that ``open -g`` suffers from.

  3. FocusMonitorApp retains the frontmost / key-window slot throughout.

Chrome vs. Safari differences
------------------------------
* ``type_text`` always takes the CGEvent fallback for Chromium inputs;
  the AX attribute-write path is silently rejected and the response says
  "via CGEvent" rather than "via AX".
* Field-value verification uses osascript against Chrome's AppleScript
  interface ("Allow JavaScript from Apple Events" must be enabled —
  setUpClass patches the Preferences JSON automatically).
* Setup uses ``launch_app`` (driver-level), not ``open -g``.

Setup (setUpClass)
------------------
1. Kill Chrome, enable "Allow JavaScript from Apple Events" (Preferences
   JSON patch), relaunch Chrome with the form URL via cua-driver's
   ``launch_app`` tool so it stays hidden.
2. Wait for the form's ``getFieldValues`` helper to be callable.
3. Launch FocusMonitorApp last — it becomes frontmost and holds focus for
   every test in the class.

Per-test design
---------------
* Prompt is intentionally minimal: capture Chrome, interact with ONE named
  element, stop.  We do NOT ask hermes to re-capture or verify — that keeps
  tool-call count to ≤ 4.
* After hermes returns the test reads the field value via osascript and
  asserts the expected value was written.
* The FocusMonitorApp focus-loss counters must not increase.

Run
---
    scripts/test.sh test_hermes_chrome_form_fill
    python3 -m pytest Tests/integration/test_hermes_chrome_form_fill.py -v
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
from driver_client import DriverClient, default_binary_path, resolve_window_id  # noqa: E402

# ---------------------------------------------------------------------------
# Paths & constants
# ---------------------------------------------------------------------------

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.dirname(os.path.dirname(_THIS_DIR))

_HTML_FORM = os.path.join(_THIS_DIR, "fixtures", "form_all_inputs.html")
_FORM_URL = f"file://{_HTML_FORM}"

_FOCUS_APP_DIR = os.path.join(_REPO_ROOT, "Tests", "FocusMonitorApp")
_FOCUS_APP_EXE = os.path.join(
    _FOCUS_APP_DIR, "FocusMonitorApp.app", "Contents", "MacOS", "FocusMonitorApp"
)
_LOSS_FILE       = "/tmp/focus_monitor_losses.txt"
_KEY_LOSS_FILE   = "/tmp/focus_monitor_key_losses.txt"
_FIELD_LOSS_FILE = "/tmp/focus_monitor_field_losses.txt"

_HERMES_BIN = os.path.expanduser("~/.hermes/hermes-agent/venv/bin/hermes")

CHROME_BUNDLE = "com.google.Chrome"

_ANTHROPIC_KEY: str = os.environ.get("ANTHROPIC_API_KEY", "")

# Use haiku for speed/cost; override with HERMES_TEST_MODEL for a stronger model.
_MODEL = os.environ.get("HERMES_TEST_MODEL", "claude-haiku-4-5-20251001")


# ---------------------------------------------------------------------------
# Chrome Apple Events JS helpers
# ---------------------------------------------------------------------------

def _enable_chrome_apple_events() -> None:
    """Quit Chrome, write allow_javascript_apple_events to all profiles.

    Mirrors the same helper in test_browser_js.py — patches Preferences JSON
    directly so we don't depend on the ``page`` tool's ``enable_javascript_apple_events``
    action (which itself needs Apple Events to already be on for the confirmation).
    Does NOT relaunch Chrome — the caller launches it via ``launch_app`` afterwards.
    """
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
            print(f"  Patched {profile}/Preferences")
        except Exception as e:
            print(f"  Skipped {profile}: {e}")


def _js_chrome(expr: str) -> str:
    """Evaluate a JS expression in Chrome's front tab via osascript.

    Uses json.dumps to safely quote the expression string for AppleScript.
    Requires "Allow JavaScript from Apple Events" to be enabled in Chrome.
    Returns stdout stripped of trailing whitespace.
    """
    # AppleScript: tell application "Google Chrome" to execute front window's
    # active tab javascript "<expr>"
    osa = (
        "tell application \"Google Chrome\" to execute "
        f"front window's active tab javascript {json.dumps(expr)}"
    )
    r = subprocess.run(
        ["osascript", "-e", osa], capture_output=True, text=True, timeout=15
    )
    return r.stdout.strip()


def _wait_for_chrome_form(timeout: float = 30.0) -> bool:
    """Wait until Chrome's active tab has the test form's key element loaded.

    Chrome's AppleScript 'execute javascript' runs in an isolated world —
    page-defined globals like getFieldValues() are NOT visible. We probe for
    the DOM element instead (document.getElementById is available in any world).
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        result = _js_chrome('document.getElementById("f-text") ? "ready" : "not-ready"')
        if result == "ready":
            return True
        time.sleep(0.5)
    return False


# Map logical field names → HTML element IDs in form_all_inputs.html.
_FIELD_TO_ID: dict[str, str] = {
    "text":     "f-text",
    "email":    "f-email",
    "password": "f-password",
    "number":   "f-number",
    "tel":      "f-tel",
    "textarea": "f-textarea",
    "select":   "f-select",
    "range":    "f-range",
    "date":     "f-date",
    "color":    "f-color",
}


def _field(name: str) -> str:
    """Read a single form field value from Chrome.

    Chrome's AppleScript 'execute javascript' runs in an isolated world, so
    page-defined helpers like getFieldValues() are not visible.  We read the
    DOM element's .value property directly instead.
    """
    elem_id = _FIELD_TO_ID.get(name, f"f-{name}")
    val = _js_chrome(
        f'(function(){{var e=document.getElementById("{elem_id}");'
        f'return e ? e.value : "__missing__";}})()'
    )
    return val if val != "__missing__" else ""


def _checkbox_checked() -> bool:
    val = _js_chrome(
        '(function(){var e=document.getElementById("f-checkbox");'
        'return e ? e.checked.toString() : "false";})()'
    )
    return val.lower() == "true"


# ---------------------------------------------------------------------------
# FocusMonitorApp helpers
# ---------------------------------------------------------------------------

def _launch_focus_app() -> tuple[subprocess.Popen, int]:
    proc = subprocess.Popen(
        [_FOCUS_APP_EXE], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
    )
    for _ in range(40):
        line = proc.stdout.readline().strip()
        if line.startswith("FOCUS_PID="):
            return proc, int(line.split("=", 1)[1])
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError("FocusMonitorApp did not print FOCUS_PID= in time")


def _read_file_int(path: str) -> int:
    try:
        with open(path) as f:
            return int(f.read().strip())
    except (FileNotFoundError, ValueError):
        return 0


# ---------------------------------------------------------------------------
# Base test class
# ---------------------------------------------------------------------------

class _HermesChromFormBase(unittest.TestCase):
    """Shared setup/teardown and hermes runner for all Chrome per-input tests."""

    _focus_proc: subprocess.Popen
    _focus_pid: int
    _chrome_pid: int
    binary: str

    @classmethod
    def setUpClass(cls) -> None:
        if not _ANTHROPIC_KEY:
            raise unittest.SkipTest(
                "ANTHROPIC_API_KEY is not set — skipping hermes Chrome form-fill tests"
            )
        if not os.path.exists(_HERMES_BIN):
            raise unittest.SkipTest(
                f"hermes not found at {_HERMES_BIN} — install hermes first"
            )

        for f in (_LOSS_FILE, _KEY_LOSS_FILE, _FIELD_LOSS_FILE):
            try:
                os.remove(f)
            except FileNotFoundError:
                pass

        # Rebuild FocusMonitorApp when Swift source is newer.
        src = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.swift")
        if (not os.path.exists(_FOCUS_APP_EXE)
                or os.path.getmtime(src) > os.path.getmtime(_FOCUS_APP_EXE)):
            subprocess.run([os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True)

        cls.binary = default_binary_path()

        # Step 1: Enable "Allow JavaScript from Apple Events" in Chrome.
        # This quits Chrome, patches every profile's Preferences JSON.
        print("\n  Enabling Chrome Apple Events JS...")
        _enable_chrome_apple_events()

        # Step 2: Launch Chrome hidden with the form URL via cua-driver's
        # launch_app — no activation, no window raise, no focus steal.
        print(f"  Launching Chrome hidden with {_FORM_URL}")
        with DriverClient(cls.binary) as c:
            result = c.call_tool("launch_app", {
                "bundle_id": CHROME_BUNDLE,
                "urls": [_FORM_URL],
            })
            sc = result.get("structuredContent", {})
            cls._chrome_pid = sc.get("pid", 0)
            if not cls._chrome_pid:
                # Fallback: extract from list_apps
                apps = c.call_tool("list_apps")["structuredContent"]["apps"]
                for app in apps:
                    if app.get("bundle_id") == CHROME_BUNDLE:
                        cls._chrome_pid = app["pid"]
                        break
        print(f"  Chrome pid: {cls._chrome_pid}")

        # Step 3: Wait for Chrome to load the form and expose getFieldValues().
        if not _wait_for_chrome_form(30.0):
            raise RuntimeError(
                f"Chrome did not load {_HTML_FORM} within 30 s.\n"
                "Check: (1) file exists, (2) 'Allow JavaScript from Apple Events' "
                "is enabled in Chrome (Develop menu)."
            )
        print("  Form loaded — f-text element ready")

        # Step 4: Launch FocusMonitorApp last — it becomes frontmost and holds
        # the focus token for every test.
        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        print(f"  FocusMonitorApp pid: {cls._focus_pid}")
        # Wait for FocusMonitorApp's focus-loss counter to stabilize.
        # The field loss counter bumps once during startup (macOS transitions
        # first-responder during the window-becoming-key sequence). We wait
        # until the counter hasn't changed for 0.5 s before recording the
        # baseline in setUp(), so startup-phase transients are not counted
        # as test failures.
        prev = _read_file_int(_FIELD_LOSS_FILE)
        for _ in range(20):   # up to 10 s
            time.sleep(0.5)
            curr = _read_file_int(_FIELD_LOSS_FILE)
            if curr == prev:
                break
            prev = curr
        print(f"  FocusMonitorApp ready (field_losses baseline: {_read_file_int(_FIELD_LOSS_FILE)})")

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        try:
            cls._focus_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            cls._focus_proc.kill()
        subprocess.run(
            ["osascript", "-e", 'quit app "Google Chrome"'],
            check=False, timeout=10,
        )

    def setUp(self) -> None:
        self._losses_before       = _read_file_int(_LOSS_FILE)
        self._key_losses_before   = _read_file_int(_KEY_LOSS_FILE)
        self._field_losses_before = _read_file_int(_FIELD_LOSS_FILE)

    def _run_hermes(self, prompt: str, timeout: int = 120, max_turns: int = 5) -> str:
        """Run hermes with the anthropic provider and return combined stdout+stderr."""
        env = os.environ.copy()
        env["ANTHROPIC_API_KEY"] = _ANTHROPIC_KEY
        env["HERMES_COMPUTER_USE_BACKEND"] = "cua"
        env["HERMES_CUA_DRIVER_CMD"] = default_binary_path()

        result = subprocess.run(
            [
                _HERMES_BIN, "chat",
                "--provider", "anthropic",
                "-m", _MODEL,
                "--toolsets", "computer_use",
                "--yolo",
                "-Q",
                "--max-turns", str(max_turns),
                "-q", prompt,
            ],
            env=env, capture_output=True, text=True, timeout=timeout,
        )
        return (result.stdout + "\n" + result.stderr).strip()

    def _assert_no_new_focus_loss(self, allow_field_losses: int = 0) -> None:
        """Assert no new app-level or key-level focus losses occurred.

        ``allow_field_losses`` controls how many text-field first-responder
        losses are tolerated. Chrome interactions trigger one field loss per
        AX action via a known ``SyntheticAppFocusEnforcer`` side effect:
        writing ``AXFocused=true`` on Chrome's window emits an AX
        ``kAXFocusedWindowChangedNotification`` that briefly disrupts
        FocusMonitorApp's first-responder chain. App-level and key-window
        focus are NOT affected (Chrome never becomes frontmost). Pass
        ``allow_field_losses=1`` for single-action Chrome tests or a higher
        value for multi-action tests.
        """
        time.sleep(0.3)
        app_delta   = _read_file_int(_LOSS_FILE)   - self._losses_before
        key_delta   = _read_file_int(_KEY_LOSS_FILE) - self._key_losses_before
        field_delta = _read_file_int(_FIELD_LOSS_FILE) - self._field_losses_before
        msgs = []
        if app_delta:
            msgs.append(f"app lost focus {app_delta}x")
        if key_delta:
            msgs.append(f"window lost key status {key_delta}x")
        if field_delta > allow_field_losses:
            msgs.append(
                f"text-input lost first-responder {field_delta}x"
                f" (allowed: {allow_field_losses})"
            )
        self.assertEqual(
            len(msgs), 0,
            f"Focus stolen in {self._testMethodName}: "
            + "; ".join(msgs)
            + " — background CGEvent delivery must not steal focus",
        )

    def _chrome_window_id(self) -> int:
        """Resolve Chrome's current on-screen window_id for AX verification."""
        with DriverClient(self.binary) as c:
            return resolve_window_id(c, self._chrome_pid)

    def _ax_contains(self, query: str) -> bool:
        """True if Chrome's AX tree contains a line matching *query* (case-insensitive).

        Uses get_window_state's query= filter so we only check the relevant
        subtree — avoids false positives from unrelated UI text.
        """
        try:
            wid = self._chrome_window_id()
            with DriverClient(self.binary) as c:
                snap = c.call_tool("get_window_state", {
                    "pid": self._chrome_pid,
                    "window_id": wid,
                    "query": query,
                })
            tree = snap.get("structuredContent", snap).get("tree_markdown", "")
            return len(tree.strip()) > 0
        except Exception:
            return False


# ---------------------------------------------------------------------------
# One test class per input type
# ---------------------------------------------------------------------------

class TestChromeTextInput(_HermesChromFormBase):
    """<input type=text> in Chrome — type_text CGEvent fallback path."""

    def test_text_input(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome' to see the form. "
            "Click the Text input (id='f-text') to focus it, "
            "then type 'Hello Chrome' into it. Stop after typing."
        )
        val = _field("text")
        self.assertEqual(val, "Hello Chrome",
                         msg=f"text='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromeEmailInput(_HermesChromFormBase):
    """<input type=email> in Chrome — CGEvent fallback for Chromium inputs."""

    def test_email_input(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome'. "
            "Click the Email input (id='f-email') and type 'agent@chrome.com'. "
            "Stop after typing."
        )
        val = _field("email")
        self.assertEqual(val, "agent@chrome.com",
                         msg=f"email='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromePasswordInput(_HermesChromFormBase):
    """<input type=password> in Chrome."""

    def test_password_input(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome'. "
            "Click the Password input (id='f-password') and type 'Secure123!'. "
            "Stop after typing."
        )
        val = _field("password")
        self.assertEqual(val, "Secure123!",
                         msg=f"password='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromeNumberInput(_HermesChromFormBase):
    """<input type=number> in Chrome."""

    def test_number_input(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome'. "
            "Click the Number input (id='f-number') and type '99'. "
            "Stop after typing."
        )
        val = _field("number")
        self.assertEqual(val, "99",
                         msg=f"number='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromeTextarea(_HermesChromFormBase):
    """<textarea> in Chrome."""

    def test_textarea(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome'. "
            "Click the Message textarea (id='f-textarea') and type 'chrome bg ok'. "
            "Stop after typing."
        )
        val = _field("textarea")
        self.assertIn("chrome bg ok", val,
                      msg=f"textarea='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromeCheckbox(_HermesChromFormBase):
    """<input type=checkbox> in Chrome — AX click."""

    def test_checkbox(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome'. "
            "Click the checkbox labelled 'I agree to the terms' (id='f-checkbox'). "
            "Stop after clicking."
        )
        self.assertTrue(_checkbox_checked(),
                        msg=f"checkbox not checked\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromeSelectDropdown(_HermesChromFormBase):
    """<select> in Chrome — set_value on AXPopUpButton.

    Chrome exposes HTML <select> as AXPopUpButton. The set_value tool finds
    the matching option AXMenuItemMarkChar and AXPresses it directly without
    opening the native macOS menu — no window raise, no focus steal.
    """

    def test_select_dropdown(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome' to see the AX tree. "
            "Find the element for 'Favourite colour (Select)' (an AXPopUpButton, id='f-select'). "
            "Use computer_use: action='set_value', element=<index>, value='Blue' to select "
            "the Blue option directly. Stop after setting.",
            max_turns=4,
            timeout=180,
        )
        val = _field("select")
        self.assertEqual(val, "blue",
                         msg=f"select='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromeSubmitButton(_HermesChromFormBase):
    """<button type=submit> in Chrome — click and verify form submission."""

    def test_submit_button(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form open. "
            "Use computer_use: action='capture', app='Google Chrome' to see the AX tree. "
            "Find the AXButton labelled 'Submit Form' (id='submit-btn') and click it. "
            "Stop after clicking."
        )
        # Chrome's AppleScript JS runs in an isolated world — check the DOM
        # result element that the form's submit handler populates instead of
        # the page-global window._submitted (which isn't visible here).
        submitted = _js_chrome(
            '(function(){var e=document.getElementById("result");'
            'return (e && e.textContent.trim().length > 0) ? "true" : "false";})()'
        )
        self.assertEqual(submitted, "true",
                         msg=f"form not submitted\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss(allow_field_losses=3)


class TestChromeFullFormFlow(_HermesChromFormBase):
    """End-to-end: fill text + email + number, then submit — all in background Chrome.

    This is the main e2e smoke test confirming:
      - launch_app correctly opens Chrome without activation
      - type_text's CGEvent fallback writes into Chromium inputs
      - Multiple tool calls across a single hermes session don't leak focus
    """

    def test_full_form_flow(self) -> None:
        out = self._run_hermes(
            "Google Chrome is in the background with a test HTML form at "
            f"'{_FORM_URL}'. "
            "Use computer_use to fill the following fields (capture Chrome first, "
            "then click each field before typing):\n"
            "  - Text input (id='f-text'): type 'E2E Test'\n"
            "  - Email input (id='f-email'): type 'e2e@test.com'\n"
            "  - Number input (id='f-number'): type '7'\n"
            "After filling all three fields, stop. "
            "Do NOT submit the form.",
            max_turns=20,
            timeout=360,
        )
        # Verify all three fields were written.
        text_val  = _field("text")
        email_val = _field("email")
        num_val   = _field("number")
        errors: list[str] = []
        if text_val  != "E2E Test":      errors.append(f"text='{text_val}'")
        if email_val != "e2e@test.com":  errors.append(f"email='{email_val}'")
        if num_val   != "7":             errors.append(f"number='{num_val}'")
        self.assertFalse(
            errors,
            f"Field mismatch: {'; '.join(errors)}\nhermes: {out[-600:]}"
        )
        self._assert_no_new_focus_loss(allow_field_losses=3)


if __name__ == "__main__":
    unittest.main(verbosity=2)
