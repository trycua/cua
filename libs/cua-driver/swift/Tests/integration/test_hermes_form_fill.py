"""Integration test: hermes CLI drives individual form inputs in backgrounded Safari.

Goal
----
One test per HTML input type. Each test asks hermes to interact with exactly
ONE element in a Safari window that is NOT frontmost — verifying that cua-driver
can synthesise events without stealing keyboard/mouse focus from the foreground app
(FocusMonitorApp).

Setup (setUpClass)
------------------
1. Open ``fixtures/form_all_inputs.html`` in Safari with ``open -g`` (background).
2. Launch FocusMonitorApp last so it owns the focus token throughout.

Per-test design
---------------
* Prompt is intentionally minimal: capture Safari, interact with ONE named element.
* We do NOT ask hermes to verify or re-capture — that keeps tool-call count to ≤ 3.
* After hermes returns, the test reads the field value via osascript JS eval and
  asserts the expected value was written.
* The FocusMonitorApp focus-loss counter must not increase.

Run
---
    scripts/test.sh test_hermes_form_fill
    python3 -m pytest Tests/integration/test_hermes_form_fill.py -v
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import default_binary_path  # noqa: E402

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
_LOSS_FILE      = "/tmp/focus_monitor_losses.txt"
_KEY_LOSS_FILE  = "/tmp/focus_monitor_key_losses.txt"
_FIELD_LOSS_FILE = "/tmp/focus_monitor_field_losses.txt"

_HERMES_BIN = os.path.expanduser("~/.hermes/hermes-agent/venv/bin/hermes")

# Deferred at module level — not evaluated until setUpClass runs. This
# avoids a KeyError during pytest --collect-only / IDE test discovery /
# lint passes that merely import the module without supplying the key.
# Each test class's setUpClass skips the entire suite when the key is absent.
_ANTHROPIC_KEY: str = os.environ.get("ANTHROPIC_API_KEY", "")

# Use haiku for speed/cost; override with HERMES_TEST_MODEL for a stronger model.
_MODEL = os.environ.get("HERMES_TEST_MODEL", "claude-haiku-4-5-20251001")

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _build_focus_app() -> None:
    if not os.path.exists(_FOCUS_APP_EXE):
        subprocess.run([os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True)


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


def _read_focus_losses() -> int:
    return _read_file_int(_LOSS_FILE)


def _read_key_losses() -> int:
    return _read_file_int(_KEY_LOSS_FILE)


def _read_field_losses() -> int:
    return _read_file_int(_FIELD_LOSS_FILE)


def _js(expr: str) -> str:
    """Evaluate a JS expression in Safari's front document via osascript."""
    script = f'tell application "Safari" to do JavaScript "{expr}" in front document'
    r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=10)
    return r.stdout.strip()


def _wait_for_form(timeout: float = 20.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _js("typeof getFieldValues") == "function":
            return True
        time.sleep(0.5)
    return False


def _field(name: str) -> str:
    raw = _js("getFieldValues()")
    try:
        return str(json.loads(raw).get(name, ""))
    except (json.JSONDecodeError, AttributeError):
        return ""


def _checkbox_checked() -> bool:
    raw = _js("getFieldValues()")
    try:
        return bool(json.loads(raw).get("checkbox", False))
    except (json.JSONDecodeError, ValueError):
        return False


# ---------------------------------------------------------------------------
# Base test class
# ---------------------------------------------------------------------------

class _HermesFormBase(unittest.TestCase):
    """Shared setup/teardown and the hermes runner for all per-input tests."""

    _focus_proc: subprocess.Popen
    _focus_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        if not _ANTHROPIC_KEY:
            raise unittest.SkipTest(
                "ANTHROPIC_API_KEY is not set — skipping hermes form-fill tests"
            )

        for f in (_LOSS_FILE, _KEY_LOSS_FILE, _FIELD_LOSS_FILE):
            if os.path.exists(f):
                os.remove(f)

        # Force rebuild FocusMonitorApp when Swift source is newer.
        src = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.swift")
        if (not os.path.exists(_FOCUS_APP_EXE)
                or os.path.getmtime(src) > os.path.getmtime(_FOCUS_APP_EXE)):
            subprocess.run([os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True)

        subprocess.run(["pkill", "-x", "Safari"], check=False)
        time.sleep(1.0)
        subprocess.run(["open", "-g", "-a", "Safari", _FORM_URL], check=True)

        if not _wait_for_form(25.0):
            raise RuntimeError(
                f"Safari did not load {_HTML_FORM} within 25 s.\n"
                "Check: (1) file exists, (2) Safari › Develop › Allow JavaScript "
                "from Apple Events is ON."
            )

        # FocusMonitorApp becomes frontmost and keeps focus throughout.
        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        time.sleep(0.5)

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        subprocess.run(["pkill", "-x", "Safari"], check=False)

    def setUp(self) -> None:
        self._losses_before       = _read_focus_losses()
        self._key_losses_before   = _read_key_losses()
        self._field_losses_before = _read_field_losses()

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
                "--yolo",        # auto-approve every computer_use action
                "-Q",            # quiet: only final response + session line
                "--max-turns", str(max_turns),
                "-q", prompt,
            ],
            env=env, capture_output=True, text=True, timeout=timeout,
        )
        return (result.stdout + "\n" + result.stderr).strip()

    def _assert_no_new_focus_loss(self) -> None:
        """Assert no new app-level OR key-level focus losses occurred."""
        time.sleep(0.3)  # let any async activation settle
        app_delta   = _read_focus_losses()  - self._losses_before
        key_delta   = _read_key_losses()    - self._key_losses_before
        field_delta = _read_field_losses()  - self._field_losses_before
        msgs = []
        if app_delta:
            msgs.append(f"app lost focus {app_delta}x")
        if key_delta:
            msgs.append(f"window lost key status {key_delta}x")
        if field_delta:
            msgs.append(f"text-input lost first-responder {field_delta}x")
        self.assertEqual(
            len(msgs), 0,
            f"Focus stolen in {self._testMethodName}: "
            + "; ".join(msgs)
            + " — background events must not steal focus",
        )


# ---------------------------------------------------------------------------
# One test class per input type
# ---------------------------------------------------------------------------

class TestTextInput(_HermesFormBase):
    """<input type=text> — fill with a short string."""

    def test_text_input(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari' to see the form. "
            "Then AX-click the Text input (id='f-text') to focus it, "
            "then type 'Hello World' into it. Stop after typing."
        )
        val = _field("text")
        self.assertEqual(val, "Hello World",
                         msg=f"text='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestPasswordInput(_HermesFormBase):
    """<input type=password> — fill with a short string."""

    def test_password_input(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari'. "
            "Then AX-click the Password input (id='f-password') and type 'Pass1234!'. "
            "Stop after typing."
        )
        val = _field("password")
        self.assertEqual(val, "Pass1234!",
                         msg=f"password='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestEmailInput(_HermesFormBase):
    """<input type=email> — fill with an email address."""

    def test_email_input(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari'. "
            "Then AX-click the Email input (id='f-email') and type 'agent@example.com'. "
            "Stop after typing."
        )
        val = _field("email")
        self.assertEqual(val, "agent@example.com",
                         msg=f"email='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestNumberInput(_HermesFormBase):
    """<input type=number> — fill with a number."""

    def test_number_input(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari'. "
            "Then AX-click the Number input (id='f-number') and type '42'. "
            "Stop after typing."
        )
        val = _field("number")
        self.assertEqual(val, "42",
                         msg=f"number='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestTextarea(_HermesFormBase):
    """<textarea> — fill with a sentence."""

    def test_textarea(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari'. "
            "Then AX-click the Message textarea (id='f-textarea') and type 'bg works'. "
            "Stop after typing."
        )
        val = _field("textarea")
        self.assertIn("bg works", val,
                      msg=f"textarea='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestCheckbox(_HermesFormBase):
    """<input type=checkbox> — check it."""

    def test_checkbox(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari'. "
            "Then AX-click the checkbox labelled 'I agree to the terms' (id='f-checkbox'). "
            "Stop after clicking."
        )
        self.assertTrue(_checkbox_checked(),
                        msg=f"checkbox not checked\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestSelectDropdown(_HermesFormBase):
    """<select> — pick an option from the dropdown.

    Strategy: click the AXPopUpButton (which now returns available options +
    a hint to use set_value), then call set_value with 'Blue'. The set_value
    tool finds the 'Blue' child option and AXPresses it directly without
    opening the native macOS popup menu.
    """

    def test_select_dropdown(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari' to see the AX tree. "
            "Find the element for 'Favourite colour (Select)' (an AXPopUpButton, id='f-select'). "
            "Use computer_use: action='set_value', element=<index>, value='Blue' to select "
            "the Blue option directly without opening the native menu. Stop after setting.",
            max_turns=4,
            timeout=180,
        )
        val = _field("select")
        self.assertEqual(val, "blue",
                         msg=f"select='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestRadioButton(_HermesFormBase):
    """<input type=radio> — select the Banana option."""

    def test_radio_button(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari'. "
            "Then AX-click the 'Banana' radio button (id='r-banana'). "
            "Stop after clicking."
        )
        val = _field("radio")
        self.assertEqual(val, "banana",
                         msg=f"radio='{val}'\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


class TestSubmitButton(_HermesFormBase):
    """<button type=submit> — click Submit and confirm the page responds."""

    def test_submit_button(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari' to see the AX tree. "
            "Find the AXButton labelled 'Submit Form' (id='submit-btn'). "
            "Click it using its element index. Stop after clicking."
        )
        submitted = _js("(window._submitted !== undefined).toString()")
        self.assertEqual(submitted, "true",
                         msg=f"form not submitted\nhermes: {out[-400:]}")
        self._assert_no_new_focus_loss()


if __name__ == "__main__":
    unittest.main(verbosity=2)
