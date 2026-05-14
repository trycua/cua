"""Binary-level API parity tests: Swift cua-driver vs Rust cua-driver-rs.

Covers every public API surface of both binaries:
  • CLI subcommands (list-tools, describe, call, mcp-config, serve/stop/status,
    recording, dump-docs, update, doctor, diagnose, config, --version, --help)
  • Stdio MCP protocol (tools/list + every registered tool)
  • Embedded daemon mode (serve socket lifecycle)

Both binaries are tested via the *same* mixin class so any divergence shows
up as a test failure attributed to the specific binary.

Environment variables:
    CUA_SWIFT_BINARY  – path to the Swift binary
                        (default: ~/.local/bin/cua-driver or `which cua-driver`)
    CUA_DRIVER_BINARY – path to the Rust binary
                        (default: ../../target/debug/cua-driver)

Run all parity tests against both binaries::

    python3 -m unittest test_api_parity -v

Run against Rust binary only::

    CUA_DRIVER_BINARY=../../target/release/cua-driver \\
        python3 -m unittest test_api_parity.RustParityTests -v

Run against Swift binary only::

    python3 -m unittest test_api_parity.SwiftParityTests -v
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)

from driver_client import DriverClient, MCPCallError, default_binary_path  # noqa: E402

# ── helpers ────────────────────────────────────────────────────────────────────

def _run(
    args: list[str],
    timeout: int = 20,
    stdin: str | None = None,
) -> subprocess.CompletedProcess:
    return subprocess.run(
        args,
        capture_output=True,
        text=True,
        timeout=timeout,
        input=stdin,
    )


def _swift_binary() -> str | None:
    """Return Swift cua-driver binary path, or None if not found."""
    from_env = os.environ.get("CUA_SWIFT_BINARY", "")
    if from_env and os.path.isfile(from_env):
        return from_env
    candidates = [
        os.path.expanduser("~/.local/bin/cua-driver"),
    ]
    found = shutil.which("cua-driver")
    if found:
        candidates.insert(0, found)
    for p in candidates:
        if os.path.isfile(p) and os.access(p, os.X_OK):
            return p
    return None


# ── tool presence lists ────────────────────────────────────────────────────────
#
# REQUIRED_TOOLS   – tools that BOTH binaries must expose (tests fail on either
#                    binary when a listed tool is absent).
# RUST_ONLY_TOOLS  – tools in Rust but not (yet) in Swift. Tests annotated
#                    with these are expected to fail on Swift.
# SWIFT_ONLY_TOOLS – tools in Swift but not (yet) registered in Rust. Tests
#                    annotated with these are expected to fail on Rust.
#
# Update these lists as parity gaps close.

REQUIRED_TOOLS = [
    # Core enumeration
    "get_screen_size",
    "screenshot",
    "list_apps",
    "list_windows",
    "get_window_state",
    # Interaction
    "click",
    "double_click",
    "right_click",
    "drag",
    "type_text",
    "press_key",
    "hotkey",
    "scroll",
    "set_value",
    "zoom",
    # App lifecycle
    "launch_app",
    # Permissions
    "check_permissions",
    # Cursor overlay
    "move_cursor",
    "get_cursor_position",
    "set_agent_cursor_style",
    "get_agent_cursor_state",
    "set_agent_cursor_enabled",
    "set_agent_cursor_motion",
    # Config
    "get_config",
    "set_config",
    # Recording / replay
    "set_recording",
    "get_recording_state",
    "replay_trajectory",
]

# In Rust, not in Swift yet.  Tests for these will fail on the Swift binary.
RUST_ONLY_TOOLS = [
    "type_text_chars",       # per-character delay typing; Swift uses type_text
    "browser_eval",          # CDP JS eval; Swift uses 'page' for browser JS
    "get_accessibility_tree",# lightweight desktop AX snapshot (separate from get_window_state)
]

# In Swift, not registered in Rust yet (page.rs exists but not in register_all).
SWIFT_ONLY_TOOLS = [
    "page",  # browser JS + get_text + query_dom; Rust uses browser_eval
]


# ── parity mixin ───────────────────────────────────────────────────────────────

class _ParityMixin:
    """All API surface tests.  Concrete subclasses set `cls.binary`."""

    binary: str = ""  # set by setUpClass

    # ── CLI: version / help ───────────────────────────────────────────────────

    def test_version_flag_exits_zero(self) -> None:
        """--version must exit 0 (even if the binary enters MCP mode silently)."""
        r = _run([self.binary, "--version"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")

    def test_version_flag_outputs_version_number(self) -> None:
        """--version must print a semver string to stdout or stderr.

        Parity gap: Rust binary currently exits 0 with no output because
        '--version' is treated as an unknown bare flag and silently enters
        MCP mode. This test documents the requirement; it will fail until
        '--version' is handled explicitly in cli::parse_command.
        """
        r = _run([self.binary, "--version"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertRegex(
            r.stdout + r.stderr,
            r"\d+\.\d+\.\d+",
            "--version did not print a semver string (N.N.N)",
        )

    def test_help_flag_exits_zero(self) -> None:
        r = _run([self.binary, "--help"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertIn("cua-driver", r.stdout + r.stderr)

    # ── CLI: list-tools ───────────────────────────────────────────────────────

    def test_list_tools_exits_zero(self) -> None:
        r = _run([self.binary, "list-tools"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")

    def test_list_tools_contains_required_tool_names(self) -> None:
        r = _run([self.binary, "list-tools"])
        self.assertEqual(r.returncode, 0)
        for name in [
            "click", "screenshot", "list_windows", "press_key",
            "get_window_state", "type_text", "hotkey", "scroll",
        ]:
            self.assertIn(name, r.stdout, f"list-tools missing: {name!r}")

    def test_list_tools_every_required_tool_present(self) -> None:
        r = _run([self.binary, "list-tools"])
        self.assertEqual(r.returncode, 0)
        for name in REQUIRED_TOOLS:
            self.assertIn(name, r.stdout, f"list-tools missing: {name!r}")

    # ── CLI: describe ─────────────────────────────────────────────────────────

    def test_describe_click_has_pid_and_schema(self) -> None:
        r = _run([self.binary, "describe", "click"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertIn("click", r.stdout)
        self.assertIn("pid", r.stdout)
        self.assertIn("input_schema", r.stdout)

    def test_describe_screenshot_has_schema(self) -> None:
        r = _run([self.binary, "describe", "screenshot"])
        self.assertEqual(r.returncode, 0)
        self.assertIn("screenshot", r.stdout)
        self.assertIn("input_schema", r.stdout)

    def test_describe_each_required_tool_exits_zero(self) -> None:
        for name in REQUIRED_TOOLS:
            with self.subTest(tool=name):
                r = _run([self.binary, "describe", name])
                self.assertEqual(
                    r.returncode, 0,
                    f"describe {name!r} exited {r.returncode}: {r.stderr}",
                )
                self.assertIn(name, r.stdout)
                self.assertIn("input_schema", r.stdout)

    def test_describe_unknown_tool_exits_64(self) -> None:
        r = _run([self.binary, "describe", "no_such_tool_xyzzy"])
        self.assertEqual(r.returncode, 64)
        self.assertIn("no_such_tool_xyzzy", r.stderr)

    # ── CLI: call (stateless tools) ───────────────────────────────────────────

    def test_call_get_screen_size_exits_zero(self) -> None:
        r = _run([self.binary, "call", "get_screen_size"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")

    def test_call_get_screen_size_returns_json(self) -> None:
        """call get_screen_size must emit JSON with width and height.

        Parity gap: Swift binary outputs human-readable text
        (e.g. "✅ Main display: 1920x1080 points @ 1.0x") instead of JSON.
        This test documents the requirement; it fails on Swift.
        """
        r = _run([self.binary, "call", "get_screen_size"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        try:
            data = json.loads(r.stdout)
        except json.JSONDecodeError:
            self.fail(
                f"call get_screen_size did not output JSON.\n"
                f"stdout: {r.stdout!r}\n"
                f"(Swift binary outputs human-readable text instead of JSON — parity gap)"
            )
        self.assertIn("width", data)
        self.assertIn("height", data)
        self.assertGreater(data["width"], 0)
        self.assertGreater(data["height"], 0)

    def test_call_screenshot_returns_b64_png(self) -> None:
        """call screenshot (no args) must return JSON with screenshot_png_b64.

        Parity gap: Swift binary requires `window_id` for screenshot and
        returns a non-JSON error when called without args.  The Rust binary
        defaults to a full-display screenshot without requiring window_id.
        This test fails on Swift on both counts; it documents the requirement.
        """
        r = _run([self.binary, "call", "screenshot"], timeout=30)
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}\nstdout: {r.stdout!r}")
        try:
            data = json.loads(r.stdout)
        except json.JSONDecodeError:
            self.fail(
                f"call screenshot did not output JSON.\n"
                f"stdout: {r.stdout!r}\n"
                f"(Swift requires window_id; Rust defaults to full display)"
            )
        self.assertIn("screenshot_png_b64", data)
        self.assertGreater(len(data["screenshot_png_b64"]), 500)

    def test_call_list_apps_returns_app_list(self) -> None:
        r = _run([self.binary, "call", "list_apps"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("apps", data)
        self.assertIsInstance(data["apps"], list)
        self.assertGreater(len(data["apps"]), 0)

    def test_call_list_apps_entries_have_required_fields(self) -> None:
        r = _run([self.binary, "call", "list_apps"])
        self.assertEqual(r.returncode, 0)
        data = json.loads(r.stdout)
        for app in data["apps"][:5]:  # spot-check first 5
            with self.subTest(app=app.get("name") or app.get("bundle_id")):
                self.assertIn("pid", app)
                self.assertIn("name", app)

    def test_call_list_windows_returns_window_list(self) -> None:
        r = _run([self.binary, "call", "list_windows"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("windows", data)
        self.assertIsInstance(data["windows"], list)

    def test_call_list_windows_entries_have_required_fields(self) -> None:
        r = _run([self.binary, "call", "list_windows"])
        self.assertEqual(r.returncode, 0)
        data = json.loads(r.stdout)
        for w in data["windows"][:5]:
            with self.subTest(window_id=w.get("window_id")):
                self.assertIn("window_id", w)
                self.assertIn("pid", w)
                self.assertIn("bounds", w)
                b = w["bounds"]
                for field in ("x", "y", "width", "height"):
                    self.assertIn(field, b)

    def test_call_check_permissions_exits_zero(self) -> None:
        r = _run([self.binary, "call", "check_permissions"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")

    def test_call_check_permissions_returns_accessibility_and_screen_recording(
        self,
    ) -> None:
        """call check_permissions must emit JSON with accessibility/screen_recording.

        Parity gap: Swift binary outputs human-readable text with emoji
        (✅/❌) instead of JSON.  This test documents the JSON requirement
        and will fail on Swift until its CLI output is standardised.
        """
        r = _run([self.binary, "call", "check_permissions"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        try:
            data = json.loads(r.stdout)
        except json.JSONDecodeError:
            self.fail(
                f"call check_permissions did not output JSON.\n"
                f"stdout: {r.stdout!r}"
            )
        self.assertIn("accessibility", data)
        self.assertIn("screen_recording", data)

    def test_call_get_cursor_position_returns_xy(self) -> None:
        r = _run([self.binary, "call", "get_cursor_position"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("x", data)
        self.assertIn("y", data)

    def test_call_get_config_returns_config_object(self) -> None:
        r = _run([self.binary, "call", "get_config"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        # Config must have at least one of the known top-level keys.
        known_keys = {
            "schema_version", "capture_mode", "max_image_dimension",
            "agent_cursor", "telemetry_enabled",
        }
        self.assertTrue(
            known_keys.intersection(data.keys()),
            f"get_config returned no known keys: {list(data.keys())}",
        )

    def test_call_get_agent_cursor_state_returns_state(self) -> None:
        r = _run([self.binary, "call", "get_agent_cursor_state"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("enabled", data)

    def test_call_get_recording_state_returns_state(self) -> None:
        r = _run([self.binary, "call", "get_recording_state"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("recording", data)

    # ── CLI: call with a pid-bearing tool (launch_app + get_window_state) ─────

    def test_call_launch_app_calculator_returns_pid(self) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)
        r = _run(
            [self.binary, "call", "launch_app", '{"bundle_id": "com.apple.calculator"}'],
            timeout=20,
        )
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("pid", data)
        self.assertGreater(data["pid"], 0)

    def test_call_get_window_state_for_calculator(self) -> None:
        """launch Calculator, get its window, then fetch get_window_state."""
        # Launch
        rl = _run(
            [self.binary, "call", "launch_app", '{"bundle_id": "com.apple.calculator"}'],
            timeout=20,
        )
        self.assertEqual(rl.returncode, 0, f"launch stderr: {rl.stderr}")
        pid = json.loads(rl.stdout)["pid"]
        time.sleep(1.0)

        # List windows to get a window_id.
        rw = _run(
            [self.binary, "call", "list_windows", f'{{"pid": {pid}}}'],
        )
        self.assertEqual(rw.returncode, 0, f"list_windows stderr: {rw.stderr}")
        windows = json.loads(rw.stdout)["windows"]
        self.assertTrue(windows, "Calculator has no windows")
        window_id = windows[0]["window_id"]

        # get_window_state
        rs = _run(
            [self.binary, "call", "get_window_state",
             f'{{"pid": {pid}, "window_id": {window_id}}}'],
            timeout=30,
        )
        self.assertEqual(rs.returncode, 0, f"get_window_state stderr: {rs.stderr}")
        data = json.loads(rs.stdout)
        self.assertIn("tree_markdown", data)
        self.assertIn("AX", data["tree_markdown"])

    # ── CLI: call — error / missing args paths ────────────────────────────────

    def test_call_unknown_tool_exits_64(self) -> None:
        r = _run([self.binary, "call", "no_such_tool_xyzzy"])
        self.assertEqual(r.returncode, 64)

    def test_call_press_key_missing_pid_exits_1(self) -> None:
        r = _run([self.binary, "call", "press_key", '{"key": "a"}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    def test_call_click_missing_required_args_exits_1(self) -> None:
        r = _run([self.binary, "call", "click", '{}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    def test_call_type_text_missing_pid_exits_1(self) -> None:
        r = _run([self.binary, "call", "type_text", '{"text": "hello"}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    def test_call_hotkey_missing_pid_exits_1(self) -> None:
        r = _run([self.binary, "call", "hotkey", '{"keys": ["cmd", "a"]}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    def test_call_scroll_missing_required_args_exits_1(self) -> None:
        r = _run([self.binary, "call", "scroll", '{}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    def test_call_drag_missing_required_args_exits_1(self) -> None:
        r = _run([self.binary, "call", "drag", '{}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    def test_call_get_window_state_missing_pid_exits_1(self) -> None:
        r = _run([self.binary, "call", "get_window_state", '{"window_id": 1}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    def test_call_launch_app_no_args_exits_1(self) -> None:
        r = _run([self.binary, "call", "launch_app", '{}'])
        self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}")

    # ── CLI: implicit call (tool name as first positional) ────────────────────

    def test_implicit_call_get_screen_size(self) -> None:
        r = _run([self.binary, "get_screen_size"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("width", data)
        self.assertIn("height", data)

    def test_implicit_call_check_permissions(self) -> None:
        r = _run([self.binary, "check_permissions"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("accessibility", data)

    def test_implicit_call_list_apps(self) -> None:
        r = _run([self.binary, "list_apps"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        data = json.loads(r.stdout)
        self.assertIn("apps", data)

    def test_implicit_call_screenshot_exits_zero(self) -> None:
        """Implicit-call screenshot must exit 0.

        Note: Swift binary requires window_id so exits non-zero with an
        error message — this is a parity gap in Swift.
        """
        r = _run([self.binary, "screenshot"], timeout=30)
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}\nstdout: {r.stdout!r}")

    def test_implicit_unknown_exits_64(self) -> None:
        r = _run([self.binary, "no_such_tool_xyzzy"])
        self.assertEqual(r.returncode, 64)

    # ── CLI: mcp-config ───────────────────────────────────────────────────────

    def test_mcp_config_default_outputs_json(self) -> None:
        r = _run([self.binary, "mcp-config"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        out = r.stdout.strip()
        # Should either be a JSON object or a shell snippet containing "cua-driver"
        self.assertIn("cua-driver", out)

    def test_mcp_config_client_claude_outputs_add_command(self) -> None:
        r = _run([self.binary, "mcp-config", "--client", "claude"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertIn("cua-driver", r.stdout)

    def test_mcp_config_client_cursor_exits_zero(self) -> None:
        r = _run([self.binary, "mcp-config", "--client", "cursor"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")

    # ── CLI: dump-docs ────────────────────────────────────────────────────────

    def test_dump_docs_exits_zero(self) -> None:
        r = _run([self.binary, "dump-docs"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertTrue(len(r.stdout) > 200, "dump-docs output too short")

    def test_dump_docs_contains_tool_names(self) -> None:
        r = _run([self.binary, "dump-docs"])
        self.assertEqual(r.returncode, 0)
        for name in ["click", "screenshot", "get_window_state"]:
            self.assertIn(name, r.stdout)

    def test_dump_docs_pretty_is_valid_json(self) -> None:
        r = _run([self.binary, "dump-docs", "--pretty"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        # --pretty outputs JSON
        try:
            data = json.loads(r.stdout)
        except json.JSONDecodeError:
            self.fail(f"dump-docs --pretty is not valid JSON:\n{r.stdout[:500]}")
        self.assertIsInstance(data, (list, dict))

    # ── CLI: diagnose ─────────────────────────────────────────────────────────

    def test_diagnose_exits_zero(self) -> None:
        r = _run([self.binary, "diagnose"], timeout=30)
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertTrue(len(r.stdout) > 50)

    def test_diagnose_mentions_accessibility(self) -> None:
        r = _run([self.binary, "diagnose"], timeout=30)
        self.assertEqual(r.returncode, 0)
        out = (r.stdout + r.stderr).lower()
        self.assertIn("access", out)

    # ── CLI: doctor ───────────────────────────────────────────────────────────

    def test_doctor_exits_zero_or_one(self) -> None:
        """doctor returns 0 if all checks pass, 1 if any check fails."""
        r = _run([self.binary, "doctor"], timeout=20)
        self.assertIn(r.returncode, (0, 1), f"unexpected exit: {r.returncode}")

    def test_doctor_produces_output(self) -> None:
        r = _run([self.binary, "doctor"], timeout=20)
        self.assertIn(r.returncode, (0, 1))
        out = r.stdout + r.stderr
        self.assertGreater(len(out.strip()), 0)

    # ── CLI: config ───────────────────────────────────────────────────────────

    def test_config_show_exits_zero(self) -> None:
        r = _run([self.binary, "config", "show"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        self.assertGreater(len(r.stdout.strip()), 0)

    def test_config_get_max_image_dimension(self) -> None:
        r = _run([self.binary, "config", "get", "max_image_dimension"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        # Value should look like an integer
        out = r.stdout.strip()
        self.assertRegex(out, r"\d+", f"expected integer, got: {out!r}")

    def test_config_get_agent_cursor_enabled(self) -> None:
        r = _run([self.binary, "config", "get", "agent_cursor.enabled"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")
        val = r.stdout.strip().lower()
        self.assertIn(val, ("true", "false"), f"unexpected value: {val!r}")

    def test_config_set_and_restore_max_image_dimension(self) -> None:
        # Read original value.
        r0 = _run([self.binary, "config", "get", "max_image_dimension"])
        self.assertEqual(r0.returncode, 0)
        original = r0.stdout.strip()

        # Set a new value.
        r1 = _run([self.binary, "config", "set", "max_image_dimension", "1024"])
        self.assertEqual(r1.returncode, 0, f"set stderr: {r1.stderr}")

        # Confirm it was applied.
        r2 = _run([self.binary, "config", "get", "max_image_dimension"])
        self.assertEqual(r2.returncode, 0)
        self.assertEqual(r2.stdout.strip(), "1024")

        # Restore original value.
        _run([self.binary, "config", "set", "max_image_dimension", original])

    def test_config_reset_exits_zero(self) -> None:
        r = _run([self.binary, "config", "reset"])
        self.assertEqual(r.returncode, 0, f"stderr: {r.stderr}")

    def test_config_get_unknown_key_exits_nonzero(self) -> None:
        r = _run([self.binary, "config", "get", "no_such_key_xyzzy"])
        self.assertNotEqual(r.returncode, 0)

    # ── CLI: update ───────────────────────────────────────────────────────────

    def test_update_dry_run_exits_zero(self) -> None:
        """update without --apply should exit 0 even if already up-to-date."""
        r = _run([self.binary, "update"], timeout=30)
        self.assertIn(r.returncode, (0, 1), f"unexpected exit: {r.returncode}")
        out = r.stdout + r.stderr
        # Should mention version or "up to date" or similar
        self.assertGreater(len(out.strip()), 0)

    # ── CLI: serve / stop / status daemon lifecycle ───────────────────────────

    def _tmp_socket(self) -> str:
        return tempfile.mktemp(suffix=".sock", prefix="cua-parity-test-")

    def test_status_exits_1_when_no_daemon(self) -> None:
        sock = self._tmp_socket()
        r = _run([self.binary, "status", "--socket", sock])
        self.assertEqual(r.returncode, 1)
        out = (r.stdout + r.stderr).lower()
        self.assertIn("not running", out)

    def test_stop_exits_1_when_no_daemon(self) -> None:
        sock = self._tmp_socket()
        r = _run([self.binary, "stop", "--socket", sock])
        self.assertEqual(r.returncode, 1)

    def test_serve_status_stop_lifecycle(self) -> None:
        sock = self._tmp_socket()
        proc = subprocess.Popen(
            [self.binary, "serve", "--socket", sock],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            # Wait for daemon to bind (up to 5 s).
            deadline = time.monotonic() + 5.0
            running = False
            while time.monotonic() < deadline:
                r = _run([self.binary, "status", "--socket", sock])
                if r.returncode == 0:
                    running = True
                    break
                time.sleep(0.1)
            self.assertTrue(running, "daemon did not become ready in 5 s")

            # status reports "running" and mentions the socket path.
            r = _run([self.binary, "status", "--socket", sock])
            self.assertEqual(r.returncode, 0, f"status stderr: {r.stderr}")
            out = (r.stdout + r.stderr).lower()
            self.assertIn("running", out)

            # stop the daemon.
            r = _run([self.binary, "stop", "--socket", sock])
            self.assertEqual(r.returncode, 0, f"stop stderr: {r.stderr}")

            # status should report not running.
            time.sleep(0.3)
            r = _run([self.binary, "status", "--socket", sock])
            self.assertEqual(r.returncode, 1)
        finally:
            proc.terminate()
            proc.wait(timeout=3)

    def test_serve_second_instance_exits_1(self) -> None:
        sock = self._tmp_socket()
        proc = subprocess.Popen(
            [self.binary, "serve", "--socket", sock],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            # Wait for first daemon.
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                r = _run([self.binary, "status", "--socket", sock])
                if r.returncode == 0:
                    break
                time.sleep(0.1)

            # Second serve should fail.
            r = _run([self.binary, "serve", "--socket", sock])
            self.assertEqual(r.returncode, 1, f"stdout: {r.stdout}\nstderr: {r.stderr}")
            out = (r.stdout + r.stderr).lower()
            self.assertIn("already running", out)
        finally:
            _run([self.binary, "stop", "--socket", sock])
            proc.wait(timeout=3)

    # ── stdio MCP: tools/list ─────────────────────────────────────────────────

    def _mcp(self) -> DriverClient:
        return DriverClient(self.binary)

    def test_mcp_tools_list_returns_all_required_tools(self) -> None:
        with self._mcp() as c:
            tools = c.list_tools()
        names = {t["name"] for t in tools}
        missing = [n for n in REQUIRED_TOOLS if n not in names]
        self.assertFalse(
            missing,
            f"tools/list missing: {missing}\n  got: {sorted(names)}",
        )

    def test_mcp_tools_list_each_has_name_description_schema(self) -> None:
        with self._mcp() as c:
            tools = c.list_tools()
        for t in tools:
            with self.subTest(tool=t.get("name")):
                self.assertIn("name", t)
                self.assertIn("description", t)
                self.assertIn("inputSchema", t)
                schema = t["inputSchema"]
                self.assertIn("type", schema)
                self.assertEqual(schema["type"], "object")

    # ── stdio MCP: stateless read tools ──────────────────────────────────────

    def test_mcp_get_screen_size(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("get_screen_size")
        sc = result.get("structuredContent", result)
        self.assertIn("width", sc)
        self.assertIn("height", sc)
        self.assertGreater(sc["width"], 0)
        self.assertGreater(sc["height"], 0)

    def test_mcp_screenshot_has_png_image(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("screenshot")
        # Should have image content.
        images = [i for i in result.get("content", []) if i.get("type") == "image"]
        sc = result.get("structuredContent", {})
        # At least one of: image in content OR screenshot_png_b64 in structuredContent.
        has_image = bool(images) or bool(sc.get("screenshot_png_b64"))
        self.assertTrue(has_image, f"screenshot returned no image: {result}")

    def test_mcp_screenshot_with_quality(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("screenshot", {"quality": 50})
        images = [i for i in result.get("content", []) if i.get("type") == "image"]
        sc = result.get("structuredContent", {})
        has_image = bool(images) or bool(sc.get("screenshot_png_b64"))
        self.assertTrue(has_image, "screenshot with quality returned no image")

    def test_mcp_list_apps(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("list_apps")
        sc = result["structuredContent"]
        self.assertIn("apps", sc)
        apps = sc["apps"]
        self.assertIsInstance(apps, list)
        self.assertGreater(len(apps), 0)
        # Every app entry must have pid + name.
        for app in apps[:5]:
            self.assertIn("pid", app)
            self.assertIn("name", app)

    def test_mcp_list_apps_has_active_flag(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("list_apps")
        apps = result["structuredContent"]["apps"]
        # At least one app should have an active field.
        has_active = any("active" in app for app in apps)
        self.assertTrue(has_active, "no app has 'active' field")

    def test_mcp_list_windows_default(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("list_windows")
        sc = result["structuredContent"]
        self.assertIn("windows", sc)
        windows = sc["windows"]
        self.assertIsInstance(windows, list)

    def test_mcp_list_windows_fields(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("list_windows")
        windows = result["structuredContent"]["windows"]
        for w in windows[:5]:
            with self.subTest(window_id=w.get("window_id")):
                self.assertIn("window_id", w)
                self.assertIn("pid", w)
                self.assertIn("bounds", w)
                for f in ("x", "y", "width", "height"):
                    self.assertIn(f, w["bounds"])

    def test_mcp_list_windows_pid_filter(self) -> None:
        """list_windows with pid filter should only return windows for that pid."""
        with self._mcp() as c:
            # Get any pid that has windows.
            all_wins = c.call_tool("list_windows")["structuredContent"]["windows"]
            if not all_wins:
                self.skipTest("no windows available")
            target_pid = all_wins[0]["pid"]
            filtered = c.call_tool("list_windows", {"pid": target_pid})
        windows = filtered["structuredContent"]["windows"]
        for w in windows:
            self.assertEqual(w["pid"], target_pid)

    def test_mcp_list_windows_on_screen_only(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("list_windows", {"on_screen_only": True})
        windows = result["structuredContent"]["windows"]
        for w in windows:
            self.assertTrue(
                w.get("is_on_screen", True),
                f"on_screen_only=true returned off-screen window: {w}",
            )

    def test_mcp_check_permissions(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("check_permissions")
        sc = result.get("structuredContent", result)
        self.assertIn("accessibility", sc)
        self.assertIn("screen_recording", sc)
        self.assertIsInstance(sc["accessibility"], bool)
        self.assertIsInstance(sc["screen_recording"], bool)

    def test_mcp_get_cursor_position(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("get_cursor_position")
        sc = result.get("structuredContent", result)
        self.assertIn("x", sc)
        self.assertIn("y", sc)

    def test_mcp_get_agent_cursor_state(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("get_agent_cursor_state")
        sc = result.get("structuredContent", result)
        self.assertIn("enabled", sc)

    def test_mcp_set_agent_cursor_enabled_toggle(self) -> None:
        with self._mcp() as c:
            # Read current.
            state = c.call_tool("get_agent_cursor_state")
            enabled = state.get("structuredContent", state).get("enabled", True)
            # Toggle off.
            c.call_tool("set_agent_cursor_enabled", {"enabled": False})
            state2 = c.call_tool("get_agent_cursor_state")
            self.assertFalse(state2.get("structuredContent", state2).get("enabled", True))
            # Restore.
            c.call_tool("set_agent_cursor_enabled", {"enabled": enabled})

    def test_mcp_set_agent_cursor_style_default(self) -> None:
        with self._mcp() as c:
            # Calling with no args or default args should succeed.
            c.call_tool("set_agent_cursor_style", {})

    def test_mcp_get_config_roundtrip(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("get_config")
        sc = result.get("structuredContent", result)
        known = {"schema_version", "capture_mode", "max_image_dimension",
                 "agent_cursor", "telemetry_enabled"}
        self.assertTrue(known.intersection(sc.keys()),
                        f"get_config has no known keys: {list(sc.keys())}")

    def test_mcp_set_config_max_image_dimension(self) -> None:
        with self._mcp() as c:
            # Read original.
            orig = c.call_tool("get_config")
            orig_dim = orig.get("structuredContent", orig).get("max_image_dimension", 1920)
            # Set new value.
            c.call_tool("set_config", {"max_image_dimension": 800})
            # Confirm.
            updated = c.call_tool("get_config")
            new_dim = updated.get("structuredContent", updated).get("max_image_dimension")
            self.assertEqual(new_dim, 800)
            # Restore.
            c.call_tool("set_config", {"max_image_dimension": orig_dim})

    def test_mcp_get_recording_state(self) -> None:
        with self._mcp() as c:
            result = c.call_tool("get_recording_state")
        sc = result.get("structuredContent", result)
        self.assertIn("recording", sc)
        self.assertIsInstance(sc["recording"], bool)

    def test_mcp_set_recording_enable_disable(self) -> None:
        import tempfile
        with tempfile.TemporaryDirectory() as tmpdir:
            with self._mcp() as c:
                # Enable recording — output_dir is required.
                c.call_tool("set_recording", {"enabled": True, "output_dir": tmpdir})
                state = c.call_tool("get_recording_state")
                sc = state.get("structuredContent", state)
                # Accept either "recording" (Swift) or "enabled" (Rust) key.
                is_recording = sc.get("recording", sc.get("enabled", False))
                self.assertTrue(is_recording, f"recording should be enabled: {sc}")
                # Disable.
                c.call_tool("set_recording", {"enabled": False})
                state2 = c.call_tool("get_recording_state")
                sc2 = state2.get("structuredContent", state2)
                is_recording2 = sc2.get("recording", sc2.get("enabled", True))
                self.assertFalse(is_recording2, f"recording should be disabled: {sc2}")

    # ── stdio MCP: tool error contracts ──────────────────────────────────────

    def _assert_tool_raises_mcp_error(
        self, tool_name: str, args: dict, timeout: float = 10.0
    ) -> None:
        """Assert that calling `tool_name` with `args` signals an error.

        Accepts either a JSON-RPC MCPCallError (Swift behaviour) or a result
        with ``isError=true`` in the tool-content list (Rust behaviour).
        """
        with self._mcp() as c:
            try:
                result = c.call_tool(tool_name, args)
                # Rust returns isError:true in the content payload
                self.assertTrue(
                    result.get("isError"),
                    f"{tool_name} did not raise an error (isError missing or false)",
                )
            except MCPCallError:
                pass  # Swift-style JSON-RPC error — acceptable

    def test_mcp_click_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("click", {"window_id": 1})

    def test_mcp_double_click_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("double_click", {"window_id": 1})

    def test_mcp_right_click_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("right_click", {"window_id": 1})

    def test_mcp_press_key_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("press_key", {"key": "a"})

    def test_mcp_hotkey_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("hotkey", {"keys": ["cmd", "a"]})

    def test_mcp_type_text_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("type_text", {"text": "hello"})

    def test_mcp_type_text_chars_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("type_text_chars", {"text": "hello"})

    def test_mcp_scroll_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("scroll", {"x": 100, "y": 100})

    def test_mcp_drag_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("drag", {})

    def test_mcp_get_window_state_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("get_window_state", {"window_id": 1})

    def test_mcp_launch_app_missing_args_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error("launch_app", {})

    def test_mcp_set_value_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error(
            "set_value", {"element_index": 0, "value": "x"}
        )

    def test_mcp_replay_trajectory_bad_dir_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error(
            "replay_trajectory", {"dir": "/tmp/no_such_trajectory_xyzzy"}
        )

    # ── stdio MCP: get_window_state live ─────────────────────────────────────

    def test_mcp_get_window_state_calculator(self) -> None:
        """Launch Calculator, capture a window state, verify tree_markdown."""
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)

        with self._mcp() as c:
            # Launch
            launch = c.call_tool("launch_app", {"bundle_id": "com.apple.calculator"})
            pid = launch.get("structuredContent", launch).get("pid", 0)
            self.assertGreater(pid, 0, f"launch_app returned bad pid: {launch}")
            time.sleep(1.0)

            # Find window
            wins = c.call_tool("list_windows", {"pid": pid})["structuredContent"]["windows"]
            self.assertTrue(wins, "Calculator has no windows")
            window_id = wins[0]["window_id"]

            # get_window_state
            state = c.call_tool("get_window_state", {"pid": pid, "window_id": window_id})

        sc = state.get("structuredContent", state)
        tree = sc.get("tree_markdown", "")
        self.assertIn("AX", tree, f"tree_markdown has no AX nodes:\n{tree}")
        # Screenshot should be present.
        images = [i for i in state.get("content", []) if i.get("type") == "image"]
        self.assertTrue(images, "get_window_state returned no screenshot")

    def test_mcp_get_window_state_has_required_sc_keys(self) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)
        with self._mcp() as c:
            launch = c.call_tool("launch_app", {"bundle_id": "com.apple.calculator"})
            pid = launch.get("structuredContent", launch).get("pid", 0)
            time.sleep(1.0)
            wins = c.call_tool("list_windows", {"pid": pid})["structuredContent"]["windows"]
            if not wins:
                self.skipTest("Calculator has no windows")
            state = c.call_tool("get_window_state", {
                "pid": pid, "window_id": wins[0]["window_id"]
            })
        sc = state.get("structuredContent", state)
        for key in ("tree_markdown", "screenshot_width", "screenshot_height"):
            self.assertIn(key, sc, f"get_window_state structuredContent missing: {key!r}")

    # ── stdio MCP: launch_app ─────────────────────────────────────────────────

    def test_mcp_launch_app_by_bundle_id(self) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)
        with self._mcp() as c:
            result = c.call_tool("launch_app", {"bundle_id": "com.apple.calculator"})
        sc = result.get("structuredContent", result)
        self.assertIn("pid", sc)
        self.assertGreater(sc["pid"], 0)

    def test_mcp_launch_app_by_name(self) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)
        with self._mcp() as c:
            result = c.call_tool("launch_app", {"name": "Calculator"})
        sc = result.get("structuredContent", result)
        self.assertIn("pid", sc)
        self.assertGreater(sc["pid"], 0)

    def test_mcp_launch_app_unknown_bundle_id_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error(
            "launch_app", {"bundle_id": "com.example.no_such_app_xyzzy"}
        )

    # ── stdio MCP: set_agent_cursor_motion ────────────────────────────────────

    def test_mcp_set_agent_cursor_motion_spring(self) -> None:
        with self._mcp() as c:
            c.call_tool("set_agent_cursor_motion", {"spring": True})

    def test_mcp_set_agent_cursor_motion_arc(self) -> None:
        with self._mcp() as c:
            c.call_tool("set_agent_cursor_motion", {"arc_size": 0.5})

    # ── stdio MCP: zoom ───────────────────────────────────────────────────────

    def test_mcp_zoom_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error(
            "zoom", {"x1": 0, "y1": 0, "x2": 100, "y2": 100}
        )

    # ── stdio MCP: page ───────────────────────────────────────────────────────

    def test_mcp_page_is_registered(self) -> None:
        """'page' tool must appear in tools/list.

        Parity gap: Rust binary has page.rs implemented but the tool is not
        yet registered in platform_macos::tools::register_all. This test will
        fail on the Rust binary until page::PageTool is added to register_all.
        """
        with self._mcp() as c:
            names = {t["name"] for t in c.list_tools()}
        self.assertIn("page", names, "'page' tool is not registered")

    def test_mcp_page_missing_pid_raises_error(self) -> None:
        self._assert_tool_raises_mcp_error(
            "page", {"action": "get_text"}
        )

    # ── stdio MCP: Rust-only tools (missing in Swift) ────────────────────────

    def test_mcp_rust_only_tools_present(self) -> None:
        """Rust-only tools (type_text_chars, browser_eval, get_accessibility_tree).

        These are in the Rust binary but not yet in Swift.  Both binaries
        should eventually have them.  This test fails on Swift until ported.
        """
        with self._mcp() as c:
            names = {t["name"] for t in c.list_tools()}
        missing = [n for n in RUST_ONLY_TOOLS if n not in names]
        self.assertFalse(
            missing,
            f"Rust-only tools missing: {missing} (not yet in this binary)",
        )

    def test_mcp_type_text_chars_missing_pid_raises_error(self) -> None:
        """type_text_chars (Rust-only) must raise MCP error with missing pid.

        Fails on Swift binary — tool not yet ported.
        """
        self._assert_tool_raises_mcp_error("type_text_chars", {"text": "hello"})

    def test_mcp_get_accessibility_tree_exits_without_pid(self) -> None:
        """get_accessibility_tree (Rust-only) returns desktop snapshot without pid.

        Fails on Swift binary — tool not yet ported.
        """
        with self._mcp() as c:
            result = c.call_tool("get_accessibility_tree")
        sc = result.get("structuredContent", result)
        # Should return some content — desktop always has windows.
        self.assertIsInstance(sc, dict)

    # ── stdio MCP: Swift-only tools (missing/unregistered in Rust) ───────────

    def test_mcp_swift_only_tools_present(self) -> None:
        """Swift-only tools (page).

        In Swift, not yet registered in Rust (page.rs exists but not in
        register_all).  Both binaries should eventually have them.
        This test fails on Rust until page::PageTool is added to register_all.
        """
        with self._mcp() as c:
            names = {t["name"] for t in c.list_tools()}
        missing = [n for n in SWIFT_ONLY_TOOLS if n not in names]
        self.assertFalse(
            missing,
            f"Swift-only tools missing: {missing} (not yet registered in this binary)",
        )

    # ── stdio MCP: browser_eval ───────────────────────────────────────────────

    def test_mcp_browser_eval_is_registered(self) -> None:
        """'browser_eval' tool must appear in tools/list.

        Parity gap: Swift binary does not have 'browser_eval' — it uses 'page'
        for browser JS. This test documents the requirement that both binaries
        expose browser_eval. Fails on Swift until it is ported.
        """
        with self._mcp() as c:
            names = {t["name"] for t in c.list_tools()}
        self.assertIn("browser_eval", names, "'browser_eval' tool is not registered")

    def test_mcp_browser_eval_missing_pid_raises_error(self) -> None:
        """browser_eval with no pid should return an MCP error."""
        self._assert_tool_raises_mcp_error(
            "browser_eval", {"expression": "1+1"}
        )

    # ── stdio MCP: protocol-level contracts ──────────────────────────────────

    def test_mcp_tools_call_unknown_tool_returns_error(self) -> None:
        """Calling an unknown tool must signal an error.

        Accepts MCPCallError (Swift / JSON-RPC error) or isError=true (Rust).
        """
        with self._mcp() as c:
            try:
                result = c.call_tool("no_such_tool_xyzzy")
                self.assertTrue(
                    result.get("isError"),
                    "unknown tool should set isError=true",
                )
            except MCPCallError:
                pass  # JSON-RPC level error — acceptable

    def test_mcp_handles_rapid_sequential_calls(self) -> None:
        """Send 5 sequential get_screen_size calls — all must succeed."""
        with self._mcp() as c:
            for i in range(5):
                result = c.call_tool("get_screen_size")
                sc = result.get("structuredContent", result)
                self.assertIn("width", sc, f"call {i} failed: {result}")

    def test_mcp_protocol_version_echo(self) -> None:
        """The initialize response should echo an MCP protocol version."""
        # The DriverClient handshake already calls initialize; if it doesn't
        # raise an exception, the server replied correctly.
        with self._mcp() as c:
            tools = c.list_tools()
        self.assertGreater(len(tools), 0)


# ── concrete test classes (one per binary) ────────────────────────────────────

class SwiftParityTests(_ParityMixin, unittest.TestCase):
    """Run the full parity suite against the Swift cua-driver binary."""

    @classmethod
    def setUpClass(cls) -> None:
        path = _swift_binary()
        if not path:
            raise unittest.SkipTest(
                "Swift cua-driver not found. "
                "Set CUA_SWIFT_BINARY or install cua-driver to ~/.local/bin/."
            )
        cls.binary = path
        print(f"\n[SwiftParityTests] binary: {cls.binary}", flush=True)


class RustParityTests(_ParityMixin, unittest.TestCase):
    """Run the full parity suite against the Rust cua-driver-rs binary."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = default_binary_path()
        if not os.path.isfile(cls.binary):
            raise unittest.SkipTest(
                f"Rust binary not found at {cls.binary}. "
                "Run `cargo build -p cua-driver` or set CUA_DRIVER_BINARY."
            )
        print(f"\n[RustParityTests] binary: {cls.binary}", flush=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
