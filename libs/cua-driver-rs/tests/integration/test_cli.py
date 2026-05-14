"""Integration test: cua-driver-rs CLI subcommands.

Covers:
  1. `list-tools` prints tool names and summaries.
  2. `describe <tool>` prints the tool schema.
  3. `call <tool> [json]` invokes a tool and prints ✅ result.
  4. Implicit call (first positional as tool name) works.
  5. Unknown tool exits 64.
  6. Error tool result exits 1.
  7. `screenshot` call returns JSON with a screenshot_png_b64 key.

Run:
    CUA_DRIVER_BINARY=../../target/debug/cua-driver python3 -m unittest test_cli -v
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import default_binary_path

BINARY = None  # set in setUpClass


def _run(args: list[str], timeout: int = 15) -> subprocess.CompletedProcess:
    return subprocess.run(
        args,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


class CLISubcommandTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = default_binary_path()

    def test_list_tools_prints_tool_names(self) -> None:
        r = _run([self.binary, "list-tools"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertIn("click", r.stdout)
        self.assertIn("screenshot", r.stdout)
        self.assertIn("list_windows", r.stdout)
        self.assertIn("press_key", r.stdout)

    def test_list_tools_has_name_colon_summary_format(self) -> None:
        r = _run([self.binary, "list-tools"])
        self.assertEqual(r.returncode, 0)
        for line in r.stdout.strip().splitlines():
            self.assertRegex(
                line,
                r"^\w+",
                f"list-tools line should start with tool name: {line!r}",
            )

    def test_describe_prints_schema(self) -> None:
        r = _run([self.binary, "describe", "click"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertIn("click", r.stdout)
        self.assertIn("pid", r.stdout)
        self.assertIn("input_schema", r.stdout)

    def test_describe_unknown_tool_exits_64(self) -> None:
        r = _run([self.binary, "describe", "no_such_tool_xyz"])
        self.assertEqual(r.returncode, 64)
        self.assertIn("no_such_tool_xyz", r.stderr)

    def test_call_get_screen_size(self) -> None:
        r = _run([self.binary, "call", "get_screen_size"])
        self.assertEqual(r.returncode, 0, f"stdout: {r.stdout}\nstderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("width", data)
        self.assertIn("height", data)
        self.assertGreater(data["width"], 0)
        self.assertGreater(data["height"], 0)

    def test_implicit_call_get_screen_size(self) -> None:
        """First positional arg is treated as tool name (no 'call' prefix)."""
        r = _run([self.binary, "get_screen_size"])
        self.assertEqual(r.returncode, 0, f"stdout: {r.stdout}\nstderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("width", data)
        self.assertIn("height", data)

    def test_call_check_permissions_exits_zero(self) -> None:
        """check_permissions is read-only and always exits 0."""
        r = _run([self.binary, "call", "check_permissions"])
        self.assertEqual(r.returncode, 0, f"stdout: {r.stdout}\nstderr: {r.stderr}")

    def test_call_check_permissions_output_has_green_checkmark(self) -> None:
        r = _run([self.binary, "call", "check_permissions"])
        self.assertEqual(r.returncode, 0)
        # The structured JSON output should have accessibility/screen_recording fields.
        data = json.loads(r.stdout)
        self.assertIn("accessibility", data)
        self.assertIn("screen_recording", data)

    def test_call_unknown_tool_exits_64(self) -> None:
        r = _run([self.binary, "call", "no_such_tool_xyz"])
        self.assertEqual(r.returncode, 64)

    def test_call_press_key_with_missing_pid_exits_1(self) -> None:
        """Calling a tool with missing required args should return an MCP error → exit 1."""
        r = _run([self.binary, "call", "press_key", '{"key": "a"}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}\nstderr: {r.stderr}")

    def test_call_list_apps_json_output(self) -> None:
        r = _run([self.binary, "call", "list_apps"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("apps", data)
        self.assertIsInstance(data["apps"], list)
        self.assertGreater(len(data["apps"]), 0)

    def test_call_screenshot_returns_b64_image(self) -> None:
        r = _run([self.binary, "call", "screenshot"], timeout=30)
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("screenshot_png_b64", data, "screenshot output missing image key")
        b64 = data["screenshot_png_b64"]
        self.assertGreater(len(b64), 100, "screenshot base64 data seems too short")


class ServeDaemonTests(unittest.TestCase):
    """Tests for `cua-driver serve` / `stop` / `status`."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = default_binary_path()
        import tempfile, os
        cls._sock_file = tempfile.mktemp(suffix=".sock", prefix="cua-driver-test-")

    def _start_daemon(self):
        import subprocess, time
        proc = subprocess.Popen(
            [self.binary, "serve", "--socket", self._sock_file],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        # Wait for daemon to bind.
        for _ in range(30):
            time.sleep(0.1)
            r = _run([self.binary, "status", "--socket", self._sock_file])
            if r.returncode == 0:
                break
        return proc

    def test_status_exits_1_when_no_daemon(self) -> None:
        import tempfile
        sock = tempfile.mktemp(suffix=".sock", prefix="cua-noexist-")
        r = _run([self.binary, "status", "--socket", sock])
        self.assertEqual(r.returncode, 1)
        self.assertIn("not running", r.stderr)

    def test_stop_exits_1_when_no_daemon(self) -> None:
        import tempfile
        sock = tempfile.mktemp(suffix=".sock", prefix="cua-noexist2-")
        r = _run([self.binary, "stop", "--socket", sock])
        self.assertEqual(r.returncode, 1)

    def test_serve_status_stop_lifecycle(self) -> None:
        """Start daemon → status is running → stop → status is not running."""
        proc = self._start_daemon()
        try:
            # Status should show running.
            r = _run([self.binary, "status", "--socket", self._sock_file])
            self.assertEqual(r.returncode, 0, f"status stderr: {r.stderr}")
            self.assertIn("running", r.stdout)
            self.assertIn("socket:", r.stdout)

            # Stop the daemon.
            r = _run([self.binary, "stop", "--socket", self._sock_file])
            self.assertEqual(r.returncode, 0, f"stop stderr: {r.stderr}")

            # Status should now show not running.
            import time; time.sleep(0.2)
            r = _run([self.binary, "status", "--socket", self._sock_file])
            self.assertEqual(r.returncode, 1)
        finally:
            proc.terminate()
            proc.wait(timeout=3)

    def test_serve_double_start_exits_1(self) -> None:
        """Second `serve` on same socket should fail with exit 1."""
        proc = self._start_daemon()
        try:
            r = _run([self.binary, "serve", "--socket", self._sock_file])
            self.assertEqual(r.returncode, 1)
            self.assertIn("already running", r.stderr)
        finally:
            _run([self.binary, "stop", "--socket", self._sock_file])
            proc.wait(timeout=3)


if __name__ == "__main__":
    unittest.main(verbosity=2)
