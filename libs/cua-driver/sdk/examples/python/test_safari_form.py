"""Example: Safari form-fill using cua_driver SDK.

Demonstrates:
  - Launching an app and navigating to a local HTML page
  - Locator.type()  — keystroke synthesis for web inputs
  - Locator.set_value() — AX direct value set for <select>
  - Locator.click()  — AX click for checkboxes and buttons
  - Verifying results via JavaScript + osascript

Setup requirements
------------------
1. Safari ›  Develop ›  Allow JavaScript from Apple Events must be ON.
2. The test HTML fixture lives at::

       Tests/integration/fixtures/form_all_inputs.html

Run::

    cd libs/cua-driver
    python -m pytest sdk/examples/python/test_safari_form.py -v
"""

from __future__ import annotations

import json
import os
import subprocess
import time

import pytest

from cua_driver import App, DriverClient, expect, kill_app


# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

_THIS_DIR      = os.path.dirname(os.path.abspath(__file__))
_CUA_DRIVER    = os.path.normpath(os.path.join(_THIS_DIR, "..", "..", ".."))
_HTML_FORM     = os.path.join(_CUA_DRIVER, "Tests", "integration",
                               "fixtures", "form_all_inputs.html")
_FORM_URL   = f"file://{_HTML_FORM}"

SAFARI_BUNDLE = "com.apple.Safari"


# ---------------------------------------------------------------------------
# JavaScript helpers (Safari Develop › Allow JS from Apple Events required)
# ---------------------------------------------------------------------------

def _js(expr: str) -> str:
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
# Module-level fixture: open Safari once, share across all tests
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def driver():
    with DriverClient() as client:
        yield client


@pytest.fixture(scope="module")
def safari(driver: DriverClient):
    """Open Safari to the test form and yield the App handle."""
    kill_app(SAFARI_BUNDLE)
    time.sleep(1.0)

    subprocess.run(["open", "-g", "-a", "Safari", _FORM_URL], check=True)
    assert _wait_for_form(25.0), (
        f"Safari did not load {_HTML_FORM} within 25s.\n"
        "Tip: Safari › Develop › Allow JavaScript from Apple Events"
    )

    # Get Safari pid via cua-driver.
    # list_apps returns text content (no structuredContent).
    import re as _re
    result = driver.call_tool("list_apps")
    text = ""
    for item in result.get("content", []):
        if item.get("type") == "text":
            text = item.get("text", "")
            break
    safari_pid = None
    for line in text.split("\n"):
        if SAFARI_BUNDLE in line:
            m = _re.search(r'\(pid (\d+)\)', line)
            if m:
                safari_pid = int(m.group(1))
                break
    assert safari_pid, f"Safari not found in app list.\nlist_apps output:\n{text}"

    app = App(driver, safari_pid, SAFARI_BUNDLE)
    yield app

    # Graceful quit in teardown — don't force-kill as it triggers macOS restore dialog
    kill_app(SAFARI_BUNDLE)


# ---------------------------------------------------------------------------
# Tests — one per form input type
# ---------------------------------------------------------------------------

def test_text_input(safari: App) -> None:
    """Type into a plain text input."""
    window = safari.main_window()
    window.locator("AXTextField", label="Text").type("Hello World")
    time.sleep(0.5)
    assert _field("text") == "Hello World", f"Got: {_field('text')!r}"


def test_password_input(safari: App) -> None:
    """Fill a password field."""
    window = safari.main_window()
    window.locator("AXTextField", label="Password").type("Pass1234!")
    time.sleep(0.5)
    assert _field("password") == "Pass1234!", f"Got: {_field('password')!r}"


def test_email_input(safari: App) -> None:
    """Fill an email input."""
    window = safari.main_window()
    window.locator("AXTextField", label="Email").type("agent@example.com")
    time.sleep(0.5)
    assert _field("email") == "agent@example.com", f"Got: {_field('email')!r}"


def test_number_input(safari: App) -> None:
    """Fill a number input."""
    window = safari.main_window()
    window.locator("AXTextField", label="Number").type("42")
    time.sleep(0.5)
    assert _field("number") == "42", f"Got: {_field('number')!r}"


def test_textarea(safari: App) -> None:
    """Fill a textarea."""
    window = safari.main_window()
    window.locator("AXTextArea", label="Message (Textarea)").type("py works")
    time.sleep(0.5)
    assert "py works" in _field("textarea"), f"Got: {_field('textarea')!r}"


def test_checkbox(safari: App) -> None:
    """Check a checkbox."""
    window = safari.main_window()
    window.locator("AXCheckBox", label="I agree to the terms").click()
    time.sleep(0.3)
    assert _checkbox_checked(), "Checkbox was not checked"


def test_select_dropdown(safari: App) -> None:
    """Select an option from a <select> dropdown via set_value."""
    window = safari.main_window()
    window.locator("AXPopUpButton", label="Favourite colour (Select)").set_value("Blue")
    time.sleep(0.3)
    assert _field("select") == "blue", f"Got: {_field('select')!r}"


def test_radio_button(safari: App) -> None:
    """Click the Banana radio button."""
    window = safari.main_window()
    window.locator("AXRadioButton", label="Banana").click()
    time.sleep(0.3)
    assert _field("radio") == "banana", f"Got: {_field('radio')!r}"


def test_submit_button(safari: App) -> None:
    """Click the form submit button."""
    window = safari.main_window()
    window.locator("AXButton", label="Submit Form").click()
    time.sleep(0.5)
    submitted = _js("(window._submitted !== undefined).toString()")
    assert submitted == "true", "Form was not submitted"
