"""Unit tests for the arc-driver arm (Amendment 9, CUA-1241): arm registration, MCP config, working directory,
argv, force-accessibility switch, tool classes, pins and the preflight's status rule, the launcher's pass-through,
and the live preflight against a stand-in MCP server started through the launcher. No GUI, no model, and nothing
from arc-cua is imported or run."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import arms  # noqa: E402
import bench_core as core  # noqa: E402
import cdb_adapter  # noqa: E402
import claude_arms as ca  # noqa: E402
import claude_events as ce  # noqa: E402
import run_bench as rb  # noqa: E402

ARM = "cc-arc-driver"
LAUNCH_C = HERE.parent / "tools" / "arc_driver" / "arc_launch.c"
REQS = HERE.parent / "tools" / "arc_driver" / "requirements-arc-0.1.1.txt"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"


def build_launcher(dest: Path) -> Path:
    clang = shutil.which("clang")
    if not clang or sys.platform != "darwin":
        raise unittest.SkipTest("needs clang on macOS")
    out = dest / "arc-launch"
    subprocess.run([clang, "-O2", "-Wall", "-Werror", "-o", str(out), str(LAUNCH_C)], check=True, capture_output=True)
    return out


class ArmRegistrationTest(unittest.TestCase):
    def test_arm_is_a_claude_arm_with_a_description_and_not_a_cua_build(self) -> None:
        self.assertIn(ARM, arms.CLAUDE_ARMS)
        self.assertIn(ARM, arms.ALL_ARMS)
        self.assertNotIn(ARM, arms.DEFAULT_CLAUDE_ARMS)  # runs only when named (follow-on arm)
        self.assertIn(ARM, ca.ARM_DESCRIPTIONS)
        self.assertIn(ARM, ca.ARC_ARMS)
        self.assertNotIn(ARM, ca.CUA_ARMS)

    def test_same_system_prompt_as_the_other_arms_and_no_skill(self) -> None:
        self.assertEqual(ca.system_prompt_for(ARM), ca.SYSTEM_PROMPT)
        with tempfile.TemporaryDirectory() as tmp:
            cwd = ca.prepare_cwd(ARM, Path(tmp) / "work")
            self.assertEqual(list(cwd.iterdir()), [])

    def test_argv_differs_only_in_the_mcp_config_and_server(self) -> None:
        common = dict(model="claude-sonnet-5-5", max_turns=45, max_budget_usd=6, effort="medium")
        arc = ca.claude_argv(mcp_config=Path("/r/mcp-cc-arc-driver.json"), server="arc", **common)
        cua = ca.claude_argv(mcp_config=Path("/r/mcp-cc-cua-driver-main.json"), server="cua", **common)
        self.assertEqual(len(arc), len(cua))
        diff = [(a, b) for a, b in zip(arc, cua) if a != b]
        self.assertEqual(diff, [("/r/mcp-cc-arc-driver.json", "/r/mcp-cc-cua-driver-main.json"), ("mcp__arc", "mcp__cua")])
        self.assertIn("--strict-mcp-config", arc)


class McpConfigTest(unittest.TestCase):
    def test_config_starts_the_pinned_venv_through_the_launcher_with_absolute_paths(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path, server = ca.mcp_config_for(ARM, Path(tmp))
            self.assertEqual(server, "arc")
            data = json.loads(path.read_text("utf-8"))
            ((name, entry),) = data["mcpServers"].items()
            self.assertEqual(name, "arc")
            self.assertEqual(entry["type"], "stdio")
            self.assertEqual(entry["command"], str(ca.ARC_LAUNCHER))
            self.assertTrue(entry["command"].endswith("ArcDriverBench.app/Contents/MacOS/arc-launch"))
            self.assertEqual(entry["args"], [str(ca.ARC_PYTHON), "-I", "-B", "-m", "arc_cua", "mcp"])
            for part in [entry["command"], *entry["args"][:1]]:
                self.assertTrue(os.path.isabs(part), part)
            self.assertNotIn("uvx", json.dumps(entry))

    def test_server_environment_is_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path, _ = ca.mcp_config_for(ARM, Path(tmp))
            env = json.loads(path.read_text("utf-8"))["mcpServers"]["arc"]["env"]
        self.assertEqual(env["HOME"], str(ca.ARC_HOME))
        self.assertEqual(env["PATH"], "/usr/bin:/bin")
        self.assertEqual(env["PYTHONNOUSERSITE"], "1")
        leaked = [k for k in env if "TOKEN" in k or "KEY" in k or k.startswith("CLAUDE") or k.startswith("CDB_")]
        self.assertEqual(leaked, [])
        self.assertNotIn(".cdb-secrets", json.dumps(env))


class ForceAccessibilityTest(unittest.TestCase):
    FLAG = "--force-renderer-accessibility"

    def test_only_the_arc_arm_gets_the_switch(self) -> None:
        self.assertEqual(ca.FORCE_ACCESSIBILITY_FLAG, self.FLAG)
        self.assertTrue(ca.force_accessibility(ARM))
        for arm in (*ca.CUA_ARMS, "cc-codex-cu"):
            self.assertFalse(ca.force_accessibility(arm), arm)

    def test_chrome_gets_it_after_the_executable_before_the_url(self) -> None:
        argv = [CHROME, "--no-first-run", "--user-data-dir=/w/p", "http://127.0.0.1:4184/"]
        out = cdb_adapter.with_force_accessibility(argv, "browser")
        self.assertEqual(out, [CHROME, self.FLAG, "--no-first-run", "--user-data-dir=/w/p", "http://127.0.0.1:4184/"])

    def test_electron_gets_it_after_its_own_arguments(self) -> None:
        exe = "/x/Lighthouse Mail.app/Contents/MacOS/Electron"
        argv = ["env", "-u", "ELECTRON_RUN_AS_NODE", exe, ".", "--surface=mail", "--workspace=/w"]
        out = cdb_adapter.with_force_accessibility(argv, "electron")
        self.assertEqual(out, [*argv, self.FLAG])
        self.assertEqual(out[4], ".")  # the app path stays where npx electron put it

    def test_other_apps_and_flagged_argv_are_unchanged(self) -> None:
        calc = ["/Applications/LibreOffice.app/Contents/MacOS/soffice", "--calc", "/w/a.ods"]
        self.assertEqual(cdb_adapter.with_force_accessibility(calc, "spreadsheet"), calc)
        node = ["node", "server.js"]
        self.assertEqual(cdb_adapter.with_force_accessibility(node, "web-server"), node)
        once = [CHROME, self.FLAG, "http://x/"]
        self.assertEqual(cdb_adapter.with_force_accessibility(once, "browser"), once)
        self.assertEqual(cdb_adapter.with_force_accessibility(["open", "-a", "Google Chrome"], "browser"),
                         ["open", "-a", "Google Chrome"])

    def test_start_apps_applies_it_only_when_asked(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            task = cdb_adapter.CdbTask.__new__(cdb_adapter.CdbTask)
            task.bundle, task.artifacts, task.log, task.procs = Path(tmp), Path(tmp), [], []
            task.workspace, task.node = Path(tmp), {"path": "node", "sha256": "", "version": ""}
            task.descriptor = {"apps": [{"id": "browser", "kind": "browser", "command": [CHROME, "http://x/"]}]}
            seen: list[list[str]] = []

            def fake_popen(argv, **_kw):
                seen.append(list(argv))
                return mock.Mock(pid=1)

            with mock.patch.object(cdb_adapter.subprocess, "Popen", fake_popen), \
                    mock.patch.object(cdb_adapter.time, "sleep", lambda _s: None):
                task.start_apps()
                task.start_apps(force_accessibility=True)
            self.assertEqual(seen[0], [CHROME, "http://x/"])
            self.assertEqual(seen[1], [CHROME, self.FLAG, "http://x/"])
            self.assertIn(f"browser: {self.FLAG}", task.log)


class ToolClassTest(unittest.TestCase):
    def test_arc_tools_and_act_actions(self) -> None:
        cases = [
            ("observe", None, "observe"),
            ("screenshot", None, "observe"),
            ("status", None, "observe"),
            ("click_at", None, "click"),
            ("drag", None, "drag"),
            ("scroll_at", None, "scroll"),
            ("press", None, "key"),
            ("type_text", None, "type"),
            ("run_command", None, "other"),
            ("act", {"action": "CLICK"}, "click"),
            ("act", {"action": "SET_VALUE"}, "set_value"),
            ("act", {"action": "TYPE_TEXT"}, "type"),
            ("act", {"action": "HOTKEY"}, "key"),
            ("act", {"action": "DRAG_TO"}, "drag"),
            ("act", {"action": "SCROLL"}, "scroll"),
            ("act", {}, "other"),
        ]
        for tool, inp, want in cases:
            self.assertEqual(ce.tool_class(f"mcp__arc__{tool}", inp), want, tool)

    def test_cua_names_are_unchanged(self) -> None:
        self.assertEqual(ce.tool_class("mcp__cua__observe"), "other")
        self.assertEqual(ce.tool_class("mcp__cua__click"), "click")
        self.assertEqual(ce.tool_class("mcp__cua__get_window_state"), "observe")


class PinsAndStatusTest(unittest.TestCase):
    def test_pins_json_names_the_pinned_release(self) -> None:
        pin = ca.load_pins()["arc_driver"]
        self.assertEqual(pin["git_sha"], "cd9b5a3bbdeb4682160ad998675723f71dab0804")
        self.assertEqual(pin["version"], "0.1.1")
        self.assertEqual(len(pin["wheel_sha256"]), 64)
        self.assertEqual(pin["bundle_id"], ca.ARC_BUNDLE_ID)
        self.assertIn(pin["wheel_sha256"], REQS.read_text("utf-8"))

    def test_requirements_are_all_pinned_with_hashes(self) -> None:
        text = REQS.read_text("utf-8")
        reqs = [line for line in text.splitlines() if line and not line.startswith((" ", "#"))]
        self.assertGreaterEqual(len(reqs), 10)
        for line in reqs:
            self.assertRegex(line, r"^[a-z0-9-]+==[0-9.]+ \\$", line)
        blocks = text.split("==")[1:]
        self.assertTrue(all("--hash=sha256:" in b for b in blocks))
        names = {r.split("==")[0] for r in reqs}
        self.assertTrue({"arc-cua", "pyobjc-core", "pyobjc-framework-quartz", "pyobjc-framework-applicationservices"} <= names)

    def test_check_arc_pins(self) -> None:
        keys = ("version", "package_tree_sha256", "site_packages_tree_sha256", "launcher_sha256", "python_version")
        pins = {"arc_driver": {k: f"v-{k}" for k in keys}}
        self.assertTrue(all(s == "pass" for _, s, _ in ca.check_arc_pins(pins, {k: f"v-{k}" for k in keys})))
        bad = ca.check_arc_pins(pins, {**{k: f"v-{k}" for k in keys}, "launcher_sha256": "other"})
        self.assertEqual([n for n, s, _ in bad if s == "fail"], ["pin arc_driver.launcher_sha256"])
        empty = ca.check_arc_pins({"arc_driver": {k: None for k in keys}}, {k: None for k in keys})
        self.assertTrue(all(s == "fail" for _, s, _ in empty))  # an unfilled pin never passes

    def test_status_rule(self) -> None:
        good = {"version": "0.1.1", "permissions": {"accessibility": True, "screen_recording": True},
                "background_input": True, "virtual_display": False}
        self.assertEqual(ca.arc_status_problems(good), [])  # the virtual display is recorded, not required
        self.assertEqual(ca.arc_status_problems({**good, "background_input": False}), ["background_input"])
        no_sr = {**good, "permissions": {"accessibility": True, "screen_recording": "error: x"}}
        self.assertEqual(ca.arc_status_problems(no_sr), ["screen_recording"])
        self.assertEqual(ca.arc_status_problems({}), list(ca.ARC_STATUS_REQUIRED))

    def test_arc_observed_reads_a_venv_without_bytecode(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            venv = Path(tmp) / "venv"
            site = venv / "lib/python3.12/site-packages"
            (site / "arc_cua/__pycache__").mkdir(parents=True)
            (site / "arc_cua/__init__.py").write_text("x = 1\n")
            (site / "arc_cua-0.1.1.dist-info").mkdir()
            (site / "arc_cua-0.1.1.dist-info/METADATA").write_text("Name: arc-cua\nVersion: 0.1.1\n")
            build = ca.ArcBuild(ARM, Path(tmp) / "none", venv / "bin/python", Path(tmp) / "home")
            first = ca.arc_observed(build)
            (site / "arc_cua/__pycache__/__init__.cpython-312.pyc").write_bytes(b"\0")
            second = ca.arc_observed(build)
            self.assertEqual(first["version"], "0.1.1")
            self.assertEqual(first, second)
            self.assertIsNone(first["launcher_sha256"])
            (site / "arc_cua/__init__.py").write_text("x = 2\n")
            self.assertNotEqual(ca.arc_observed(build)["package_tree_sha256"], first["package_tree_sha256"])


class LauncherTest(unittest.TestCase):
    def test_passes_stdio_through_and_returns_the_exit_status(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            launcher = build_launcher(Path(tmp))
            done = subprocess.run([str(launcher), "/bin/cat"], input="line 1\nline 2\n", capture_output=True, text=True, timeout=20)
            self.assertEqual((done.returncode, done.stdout), (0, "line 1\nline 2\n"))
            done = subprocess.run([str(launcher), "/bin/sh", "-c", "echo err >&2; exit 7"], capture_output=True, text=True, timeout=20)
            self.assertEqual((done.returncode, done.stderr), (7, "err\n"))

    def test_program_runs_two_levels_down_in_the_same_process_group_without_the_stage_variable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            launcher = build_launcher(Path(tmp))
            script = 'echo "${ARC_LAUNCH_STAGE:-unset}"; ps -o pgid= -p $$; ps -o pgid= -p $PPID; ps -o command= -p $PPID'
            done = subprocess.run([str(launcher), "/bin/sh", "-c", script], capture_output=True, text=True, timeout=20)
            stage, pgid, parent_pgid, parent = [x.strip() for x in done.stdout.splitlines()]
            self.assertEqual(stage, "unset")
            self.assertEqual(pgid, parent_pgid)
            self.assertIn("arc-launch", parent)

    def test_missing_program_fails_cleanly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            launcher = build_launcher(Path(tmp))
            done = subprocess.run([str(launcher), "/nonexistent/program"], capture_output=True, text=True, timeout=20)
            self.assertEqual(done.returncode, 127)
            done = subprocess.run([str(launcher)], capture_output=True, text=True, timeout=20)
            self.assertEqual(done.returncode, 64)


FAKE_SERVER = textwrap.dedent(
    """
    import json, sys
    TOOLS = {tools!r}
    STATUS = {status!r}
    for line in sys.stdin:
        msg = json.loads(line)
        if "id" not in msg:
            continue
        method = msg.get("method")
        if method == "initialize":
            result = {{"protocolVersion": "2025-06-18", "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "fake"}}}}
        elif method == "tools/list":
            result = {{"tools": [{{"name": t, "inputSchema": {{"type": "object"}}}} for t in TOOLS]}}
        elif method == "tools/call":
            result = {{"content": [{{"type": "text", "text": json.dumps(STATUS)}}], "structuredContent": STATUS}}
        else:
            result = {{}}
        sys.stdout.write(json.dumps({{"jsonrpc": "2.0", "id": msg["id"], "result": result}}) + "\\n")
        sys.stdout.flush()
    """
)


class LivePreflightTest(unittest.TestCase):
    """arc_live_checks against a stand-in MCP server started through the real launcher (not arc-cua)."""

    def run_checks(self, tools: list[str], status: dict) -> list[tuple[str, str, str]]:
        with tempfile.TemporaryDirectory() as tmp:
            t = Path(tmp)
            launcher = build_launcher(t)
            server = t / "fake_server.py"
            server.write_text(FAKE_SERVER.format(tools=tools, status=status), "utf-8")
            config = t / "mcp-cc-arc-driver.json"
            config.write_text(json.dumps({"mcpServers": {"arc": {
                "type": "stdio", "command": str(launcher), "args": [sys.executable, "-I", str(server)],
                "env": {"HOME": str(t), "PATH": "/usr/bin:/bin"}}}}), "utf-8")
            args = argparse.Namespace(ledger_who="t", phase1_runs=1, phase2_runs=0, schedule_seed=1)
            ctx = rb.Ctx(args, t, core.Ledger(t / "l.jsonl"), {}, ["MB-10"], [ARM])
            ctx.mcp[ARM] = (config, "arc")
            with mock.patch.object(ca, "load_pins", return_value={"arc_driver": {"version": "0.1.1"}}):
                return rb.arc_live_checks(ctx, ARM)

    GOOD = {"version": "0.1.1", "permissions": {"accessibility": True, "screen_recording": True},
            "background_input": True, "virtual_display": False}

    def test_passes_with_all_tools_and_permissions(self) -> None:
        checks = self.run_checks(list(ca.ARC_TOOLS), self.GOOD)
        self.assertEqual([s for _, s, _ in checks], ["pass", "pass", "pass", "warn"], checks)

    def test_fails_without_screen_recording_or_with_a_wrong_tool_list(self) -> None:
        checks = self.run_checks(list(ca.ARC_TOOLS), {**self.GOOD, "permissions": {"accessibility": True, "screen_recording": False}})
        self.assertEqual(checks[1][1], "fail")
        self.assertIn("screen_recording", checks[1][2])
        checks = self.run_checks([*ca.ARC_TOOLS, "launch_app"], self.GOOD)
        self.assertEqual(checks[0][1], "fail")


if __name__ == "__main__":
    unittest.main()
