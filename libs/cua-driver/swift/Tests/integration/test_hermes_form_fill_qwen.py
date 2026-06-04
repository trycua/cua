"""Integration test: hermes CLI drives form inputs using the local Qwen mlx-vlm server.

Same setup as test_hermes_form_fill.py but uses the local mlx-vlm server
(http://127.0.0.1:8080/v1, model mlx-community/Qwen3.6-35B-A3B-4bit) instead
of the Anthropic API.  Starts with a single test (text input) to validate the
multimodal tool-result fix that unwraps _multimodal dicts to OpenAI content
lists so vision-capable OpenAI-compatible servers can handle images in tool
messages.

Run:
    python3 -m pytest Tests/integration/test_hermes_form_fill_qwen.py -v
    # or pick one test:
    python3 -m pytest Tests/integration/test_hermes_form_fill_qwen.py::TestQwenTextInput -v
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
_LOSS_FILE       = "/tmp/focus_monitor_losses.txt"
_KEY_LOSS_FILE   = "/tmp/focus_monitor_key_losses.txt"
_FIELD_LOSS_FILE = "/tmp/focus_monitor_field_losses.txt"

_HERMES_BIN = os.path.expanduser("~/.hermes/hermes-agent/venv/bin/hermes")

# Local mlx-vlm server endpoint for the skip check.
# The model/provider are read from the user's hermes config.yaml (where
# provider: custom + base_url are set), so we don't pass --provider/-m
# flags — hermes picks up the configured defaults automatically.
_ENDPOINT = os.environ.get("HERMES_QWEN_ENDPOINT", "http://127.0.0.1:8080/v1")

# ---------------------------------------------------------------------------
# Helpers (identical to test_hermes_form_fill.py)
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


# ---------------------------------------------------------------------------
# Base test class
# ---------------------------------------------------------------------------

class _QwenFormBase(unittest.TestCase):
    """Shared setup/teardown for Qwen-backed hermes form-fill tests."""

    _focus_proc: subprocess.Popen
    _focus_pid: int

    @classmethod
    def setUpClass(cls) -> None:
        # Verify the local server is reachable before spending time on setup.
        import urllib.request, urllib.error
        try:
            urllib.request.urlopen(_ENDPOINT.rstrip("/") + "/models", timeout=3)
        except urllib.error.URLError as e:
            raise unittest.SkipTest(
                f"Local mlx-vlm server not reachable at {_ENDPOINT}: {e}. "
                "Start it with: mlx_vlm.server --model mlx-community/Qwen3.6-35B-A3B-4bit"
            )

        for f in (_LOSS_FILE, _KEY_LOSS_FILE, _FIELD_LOSS_FILE):
            if os.path.exists(f):
                os.remove(f)

        src = os.path.join(_FOCUS_APP_DIR, "FocusMonitorApp.swift")
        if (not os.path.exists(_FOCUS_APP_EXE)
                or os.path.getmtime(src) > os.path.getmtime(_FOCUS_APP_EXE)):
            subprocess.run([os.path.join(_FOCUS_APP_DIR, "build.sh")], check=True)

        subprocess.run(["pkill", "-x", "Safari"], check=False)
        time.sleep(1.0)
        subprocess.run(["open", "-g", "-a", "Safari", _FORM_URL], check=True)

        if not _wait_for_form(25.0):
            raise RuntimeError(f"Safari did not load {_HTML_FORM} within 25 s.")

        cls._focus_proc, cls._focus_pid = _launch_focus_app()
        time.sleep(0.5)

    @classmethod
    def tearDownClass(cls) -> None:
        cls._focus_proc.terminate()
        subprocess.run(["pkill", "-x", "Safari"], check=False)

    def setUp(self) -> None:
        self._losses_before       = _read_file_int(_LOSS_FILE)
        self._key_losses_before   = _read_file_int(_KEY_LOSS_FILE)
        self._field_losses_before = _read_file_int(_FIELD_LOSS_FILE)

    def _run_hermes(self, prompt: str, timeout: int = 180, max_turns: int = 6) -> str:
        env = os.environ.copy()
        env["HERMES_COMPUTER_USE_BACKEND"] = "cua"
        env["HERMES_CUA_DRIVER_CMD"] = default_binary_path()

        # No --provider or -m flags: hermes reads them from ~/.hermes/config.yaml
        # (provider: custom, base_url: http://127.0.0.1:8080/v1, model: Qwen3.6…)
        result = subprocess.run(
            [
                _HERMES_BIN, "chat",
                "--toolsets", "computer_use",
                "--yolo",
                "-Q",
                "--max-turns", str(max_turns),
                "-q", prompt,
            ],
            env=env, capture_output=True, text=True, timeout=timeout,
        )
        return (result.stdout + "\n" + result.stderr).strip()

    def _assert_no_new_focus_loss(self) -> None:
        time.sleep(0.3)
        app_delta   = _read_file_int(_LOSS_FILE)   - self._losses_before
        key_delta   = _read_file_int(_KEY_LOSS_FILE) - self._key_losses_before
        field_delta = _read_file_int(_FIELD_LOSS_FILE) - self._field_losses_before
        msgs = []
        if app_delta:   msgs.append(f"app lost focus {app_delta}x")
        if key_delta:   msgs.append(f"window lost key status {key_delta}x")
        if field_delta: msgs.append(f"text-input lost first-responder {field_delta}x")
        self.assertEqual(len(msgs), 0,
            f"Focus stolen in {self._testMethodName}: " + "; ".join(msgs))


# ---------------------------------------------------------------------------
# Tests — start with text input only; add more once text passes
# ---------------------------------------------------------------------------

class TestQwenTextInput(_QwenFormBase):
    """<input type=text> — verifies multimodal tool results work with mlx-vlm."""

    def test_text_input(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari' to see the form. "
            "Then AX-click the Text input (id='f-text') to focus it, "
            "then type 'Hello World' into it. Stop after typing."
        )
        val = _field("text")
        self.assertEqual(val, "Hello World",
                         msg=f"text='{val}'\nhermes: {out[-600:]}")
        self._assert_no_new_focus_loss()


class TestQwenEmailInput(_QwenFormBase):
    """<input type=email>"""

    def test_email_input(self) -> None:
        out = self._run_hermes(
            "Safari is in the background with a test form open. "
            "Use computer_use: action='capture', app='Safari'. "
            "Then AX-click the Email input (id='f-email') and type 'agent@example.com'. "
            "Stop after typing."
        )
        val = _field("email")
        self.assertEqual(val, "agent@example.com",
                         msg=f"email='{val}'\nhermes: {out[-600:]}")
        self._assert_no_new_focus_loss()


if __name__ == "__main__":
    unittest.main(verbosity=2)
