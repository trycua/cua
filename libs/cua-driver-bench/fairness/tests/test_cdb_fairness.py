# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Tests of the runner-agnostic fairness and health checks (no GUI, no VM, no model)."""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import tempfile
import textwrap
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import cdb_fairness as fx  # noqa: E402

FAKE_MCP = textwrap.dedent(
    """
    import json, sys
    mode = sys.argv[1]
    for line in sys.stdin:
        msg = json.loads(line)
        if "id" not in msg:
            continue
        if mode == "silent":
            continue
        if msg["method"] == "initialize":
            print("a log line, not JSON", flush=True)
            out = {"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "fake", "version": "1"}}
        elif msg["method"] == "tools/list":
            out = {"tools": [] if mode == "notools" else [{"name": "check_permissions", "inputSchema": {}}]}
        elif msg["method"] == "tools/call":
            out = {"content": [{"type": "text", "text": "ok"}], "isError": mode == "probefail"}
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": out}), flush=True)
    """
)


class SeedTests(unittest.TestCase):
    def test_matches_the_runner_seed(self) -> None:
        sys.path.insert(0, str(HERE.parents[1] / "automated-eval" / "macos_bench"))
        import bench_core

        for task in ("MB-09", "IR-01", "CDB-G02", "CDB-G04"):
            for run_index in range(6):
                self.assertEqual(
                    fx.trial_seed(task, run_index), bench_core.probe_seed(task, run_index)
                )

    def test_known_value_and_range(self) -> None:
        self.assertEqual(fx.trial_seed("MB-09", 0), 790113)
        self.assertTrue(all(0 <= fx.trial_seed("IR-02", i) < 1_000_000 for i in range(50)))
        self.assertNotEqual(fx.trial_seed("IR-02", 0), fx.trial_seed("IR-02", 1))

    def test_negative_run_index_rejected(self) -> None:
        with self.assertRaises(ValueError):
            fx.trial_seed("MB-09", -1)


class McpPreflightTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.server = self.tmp / "fake_mcp.py"
        self.server.write_text(FAKE_MCP)

    def run_mode(self, mode: str, **kw: object) -> fx.Check:
        return fx.mcp_preflight([sys.executable, str(self.server), mode], timeout=5, **kw)

    def test_ready_server_passes(self) -> None:
        check = self.run_mode("ok")
        self.assertTrue(check.ok, check)
        self.assertEqual(check.detail["tools"], 1)

    def test_probe_tool(self) -> None:
        self.assertTrue(self.run_mode("ok", probe_tool="check_permissions").ok)
        self.assertFalse(self.run_mode("probefail", probe_tool="check_permissions").ok)
        self.assertIn("not offered", self.run_mode("ok", probe_tool="nope").reason)

    def test_no_tools_is_a_harness_error(self) -> None:
        self.assertFalse(self.run_mode("notools").ok)

    def test_silent_server_times_out(self) -> None:
        check = fx.mcp_preflight([sys.executable, str(self.server), "silent"], timeout=1)
        self.assertFalse(check.ok)
        self.assertIn("initialize", check.reason)

    def test_missing_binary(self) -> None:
        self.assertFalse(fx.mcp_preflight([str(self.tmp / "absent")], timeout=1).ok)

    def test_from_config(self) -> None:
        config = self.tmp / "mcp.json"
        config.write_text(
            json.dumps(
                {
                    "mcpServers": {
                        "good": {"command": sys.executable, "args": [str(self.server), "ok"]},
                        "remote": {"type": "http", "url": "http://127.0.0.1:9/mcp"},
                    }
                }
            )
        )
        checks = fx.mcp_preflight_from_config(config, timeout=5)
        self.assertEqual([c.ok for c in checks], [True, False])
        self.assertEqual(len(fx.mcp_preflight_from_config(config, server="good", timeout=5)), 1)
        self.assertFalse(fx.mcp_preflight_from_config(config, server="absent")[0].ok)

    def test_cli_exit_codes(self) -> None:
        config = self.tmp / "mcp.json"
        config.write_text(
            json.dumps(
                {
                    "mcpServers": {
                        "s": {"command": sys.executable, "args": [str(self.server), "notools"]}
                    }
                }
            )
        )
        done = subprocess.run(
            [
                sys.executable,
                str(HERE.parent / "cdb_fairness.py"),
                "mcp",
                str(config),
                "--timeout",
                "5",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(done.returncode, fx.EXIT_HARNESS_ERROR)
        self.assertFalse(json.loads(done.stdout)["ok"])


class SessionInitTests(unittest.TestCase):
    def test_connected(self) -> None:
        event = {
            "type": "system",
            "subtype": "init",
            "mcp_servers": [{"name": "cu", "status": "connected"}],
        }
        self.assertTrue(fx.session_init_from_stream(["noise", json.dumps(event)]).ok)

    def test_failed_server(self) -> None:
        event = {
            "type": "system",
            "subtype": "init",
            "mcp_servers": [{"name": "cu", "status": "failed"}],
        }
        check = fx.session_init_from_stream([json.dumps(event)])
        self.assertFalse(check.ok)
        self.assertIn("cu", check.reason)

    def test_no_server_or_no_init(self) -> None:
        self.assertFalse(fx.session_servers_connected({"mcp_servers": []}).ok)
        self.assertFalse(fx.session_init_from_stream(['{"type": "assistant"}']).ok)


class AppleEventsTests(unittest.TestCase):
    def fake(self, rc: int, stderr: str = "") -> mock._patch:
        return mock.patch.object(
            fx.subprocess, "run", return_value=subprocess.CompletedProcess([], rc, "", stderr)
        )

    def test_ok(self) -> None:
        with self.fake(0):
            self.assertTrue(fx.appleevents_health(["Calendar"])[0].ok)

    def test_not_authorized_is_a_harness_error(self) -> None:
        with self.fake(
            1, "execution error: Error: Not authorized to send Apple events to Calendar. (-1743)"
        ):
            check = fx.appleevents_health(["Calendar"])[0]
        self.assertFalse(check.ok)
        self.assertEqual(check.detail["code"], -1743)
        self.assertIn("consent", check.reason)

    def test_not_running_app_is_not_launched(self) -> None:
        done = subprocess.CompletedProcess([], 0, "__not_running__\n", "")
        with mock.patch.object(fx.subprocess, "run", return_value=done) as run:
            check = fx.appleevents_health(["TextEdit"])[0]
        self.assertTrue(check.ok)
        self.assertFalse(check.detail["checked"])
        self.assertIn("running()", run.call_args[0][0][-1])
        with mock.patch.object(fx.subprocess, "run", return_value=done) as run:
            fx.appleevents_health(["TextEdit"], launch=True)
        self.assertNotIn("running()", run.call_args[0][0][-1])

    def test_timeout(self) -> None:
        with mock.patch.object(
            fx.subprocess, "run", side_effect=subprocess.TimeoutExpired("osascript", 1)
        ):
            self.assertFalse(fx.appleevents_health(["Notes"])[0].ok)


class _Healthz(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        self.send_response(200 if self.path == "/healthz" else 404)
        self.end_headers()

    def log_message(self, *args: object) -> None:
        pass


class NetworkTests(unittest.TestCase):
    def test_loopback(self) -> None:
        self.assertTrue(fx.loopback_health().ok)

    def test_ready_url(self) -> None:
        server = HTTPServer(("127.0.0.1", 0), _Healthz)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            base = f"http://127.0.0.1:{server.server_port}"
            self.assertTrue(fx.http_ready(base + "/healthz", timeout=3).ok)
            self.assertFalse(fx.http_ready(base + "/missing", timeout=1).ok)
        finally:
            server.shutdown()
            server.server_close()

    def test_lease(self) -> None:
        with mock.patch.object(fx, "primary_ipv4", return_value="169.254.10.2"):
            self.assertIn("self-assigned", fx.lease_health().reason)
        with mock.patch.object(fx, "primary_ipv4", return_value=None):
            self.assertFalse(fx.lease_health().ok)
        with mock.patch.object(fx, "primary_ipv4", return_value="192.0.2.5"):
            self.assertTrue(fx.lease_health().ok)

    def test_loopback_failure(self) -> None:
        with mock.patch.object(fx.socket, "socket", side_effect=OSError("no lo0")):
            self.assertFalse(fx.loopback_health().ok)


class ToolchainTests(unittest.TestCase):
    DESCRIPTOR = {
        "semantics": {
            "reset": {
                "setup": ["${python}", "reset/setup.py"],
                "verify": ["${python}", "reset/verify.py"],
            },
            "export": ["${python}", "reset/export.py"],
            "evaluate": ["${python}", "evaluator/evaluate.py", "--node", "${evaluator_node}"],
        }
    }

    def test_programs_of_descriptor(self) -> None:
        self.assertEqual(fx.programs_of_descriptor(self.DESCRIPTOR), ["python3", "node"])
        self.assertEqual(
            fx.programs_of_descriptor(self.DESCRIPTOR, {"python": "/opt/py"}), ["/opt/py", "node"]
        )

    def test_missing_program_is_a_harness_error(self) -> None:
        with tempfile.TemporaryDirectory() as empty:
            checks = fx.toolchain_health(["node"], path=empty)
        self.assertFalse(checks[0].ok)
        self.assertIn("not found", checks[0].reason)

    def test_present_program(self) -> None:
        with tempfile.TemporaryDirectory() as bindir:
            tool = Path(bindir) / "xattr"
            tool.write_text("#!/bin/sh\nexit 0\n")
            tool.chmod(0o755)
            self.assertTrue(fx.toolchain_health(["xattr"], path=bindir)[0].ok)
        self.assertTrue(fx.toolchain_health([sys.executable])[0].ok)


class WatchdogTests(unittest.TestCase):
    def test_lost_after_consecutive_misses(self) -> None:
        answers = iter([True, False, False, True, False, False, False])
        dog = fx.EnvWatchdog(lambda: next(answers), interval=999, misses=3)
        for _ in range(7):
            dog.tick()
        check = dog.stop()
        self.assertFalse(check.ok)
        self.assertEqual(check.detail["probes"], 7)

    def test_raising_probe_counts_as_a_miss_and_healthy_env_passes(self) -> None:
        def boom() -> bool:
            raise OSError("gone")

        dog = fx.EnvWatchdog(boom, interval=999, misses=2)
        dog.tick()
        dog.tick()
        self.assertFalse(dog.stop().ok)
        healthy = fx.EnvWatchdog(lambda: True, interval=0.01, misses=2).start()
        threading.Event().wait(0.1)
        self.assertTrue(healthy.stop().ok)


class OutcomeTests(unittest.TestCase):
    def row(self, task: str, run_index: int, **kw: object) -> dict:
        return {"arm": "a", "task": task, "run_index": run_index, "condition": "foreground", **kw}

    def test_classify(self) -> None:
        self.assertEqual(fx.classify(self.row("T", 0, passed=True)), fx.PASS)
        self.assertEqual(fx.classify(self.row("T", 0, passed=False)), fx.FAIL)
        self.assertEqual(
            fx.classify(self.row("T", 0, passed=True, harness_error="export_failed")),
            fx.HARNESS_ERROR,
        )
        self.assertEqual(fx.classify(self.row("T", 0, status="infra_error")), fx.HARNESS_ERROR)
        self.assertEqual(
            fx.classify(self.row("T", 0, passed=True, health=[{"ok": False}])), fx.HARNESS_ERROR
        )

    def test_harness_errors_are_never_scored_and_rerun_with_the_same_seed(self) -> None:
        rows = [
            self.row("T", 0, harness_error="mcp_preflight"),
            self.row("T", 0, passed=False),  # the re-run counts
            self.row("U", 0, harness_error="env_lost"),
            self.row("V", 1, harness_error="env_lost"),
            self.row("V", 1, harness_error="env_lost"),
            self.row("V", 1, harness_error="env_lost"),
        ]
        scored = fx.scored_rows(rows)
        self.assertEqual([(r["task"], r["passed"]) for r in scored], [("T", False)])
        plan = fx.rerun_plan(rows, max_attempts=3)
        self.assertEqual(
            [(i["task"], i["seed"]) for i in plan["rerun"]], [("U", fx.trial_seed("U", 0))]
        )
        self.assertEqual([i["task"] for i in plan["excluded"]], ["V"])
        summary = fx.summarize(rows)
        self.assertEqual(
            (summary["scored"], summary["harness_error_attempts"], summary["passed"]), (1, 5, 0)
        )

    def test_summarize_cli(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "rows.jsonl"
            path.write_text(
                "\n".join(json.dumps(r) for r in [self.row("T", 0, passed=True)]) + "\n"
            )
            done = subprocess.run(
                [sys.executable, str(HERE.parent / "cdb_fairness.py"), "summarize", str(path)],
                capture_output=True,
                text=True,
                env={**os.environ, "PYTHONPATH": ""},
            )
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(json.loads(done.stdout)["passed"], 1)


class LoopbackSocketSanity(unittest.TestCase):
    def test_primary_ipv4_never_raises(self) -> None:
        with mock.patch.object(fx.socket, "socket", side_effect=OSError):
            self.assertIsNone(fx.primary_ipv4())
        self.assertIsInstance(socket.AF_INET, int)


if __name__ == "__main__":
    unittest.main()
