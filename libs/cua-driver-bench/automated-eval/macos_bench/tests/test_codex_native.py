# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Amendment 20 (CUA-1347): the native Codex arm (Codex CLI + gpt-6-astra driving Codex computer use)."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import arms  # noqa: E402
import bench_v2 as v2  # noqa: E402
import claude_events  # noqa: E402
import codex_native as cxn  # noqa: E402

ARM_B = {
    "mcpServers": {
        "codex-cu": {
            "type": "stdio",
            "command": "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node",
            "args": [
                "/Applications/ChatGPT.app/Contents/Resources/cua_node/lib/node_modules/@oai/cua-repl/bin/cua-repl.mjs"
            ],
            "env": {
                "CUA_REPL_ENABLED_SURFACES": "computer",
                "CODEX_HOME": "/Users/lume/.codex",
                "X": "1",
            },
        }
    }
}


def write_arm_b(folder: Path, surfaces: str = "computer") -> Path:
    data = json.loads(json.dumps(ARM_B))
    data["mcpServers"]["codex-cu"]["env"]["CUA_REPL_ENABLED_SURFACES"] = surfaces
    path = folder / "mcp.json"
    path.write_text(json.dumps(data), "utf-8")
    return path


class HomeTest(unittest.TestCase):
    def test_auth_modes_preserve_config_and_share_credentials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / ".codex/auth.json"
            source.parent.mkdir()
            source.write_bytes(b"private")
            source.chmod(0o000)
            before = source.stat().st_mtime_ns
            mcp = write_arm_b(root)
            for arm in cxn.ARMS:
                configs = {}
                for mode in ("gateway", "chatgpt"):
                    with mock.patch.dict(
                        os.environ, {"CDB_CODEX_AUTH": mode, "HOME": tmp}, clear=True
                    ):
                        env = cxn.render_home(
                            root / f"{arm}-{mode}", mcp, cxn.MODEL, cxn.EFFORT, "SYSTEM", arm
                        )
                        home = Path(env["CODEX_HOME"])
                        configs[mode] = tomllib.loads((home / "config.toml").read_text())
                        link = home / "auth.json"
                        self.assertEqual(link.is_symlink(), mode == "chatgpt")
                        if mode == "chatgpt":
                            self.assertEqual(os.readlink(link), str(source))
                            arms.link_chatgpt_auth(home)
                            link.unlink()
                            link.symlink_to(root / "other")
                            with self.assertRaises(FileExistsError):
                                arms.link_chatgpt_auth(home)
                            link.unlink()
                            link.write_text("conflict")
                            with self.assertRaises(FileExistsError) as conflict:
                                arms.link_chatgpt_auth(home)
                            self.assertNotIn(str(source), str(conflict.exception))
                gateway = configs["gateway"]
                self.assertEqual(gateway.pop("model_provider"), "litellm")
                self.assertIn("litellm", gateway.pop("model_providers"))
                self.assertEqual(gateway, configs["chatgpt"])
                self.assertEqual(gateway["cli_auth_credentials_store"], "file")
                self.assertEqual(gateway["model"], "gpt-6.1-sol")
                self.assertEqual(gateway["model_reasoning_effort"], "low")
            with mock.patch.dict(
                os.environ, {"CDB_CODEX_AUTH": "chatgpt", "CDB_CODEX_AUTH_FILE": str(source)}
            ):
                env = cxn.render_home(root / "override", mcp, cxn.MODEL, cxn.EFFORT, "SYSTEM")
                self.assertEqual(os.readlink(Path(env["CODEX_HOME"]) / "auth.json"), str(source))
                self.assertEqual(source.stat().st_mtime_ns, before)
                source.chmod(0o600)
                self.assertEqual(source.read_bytes(), b"private")
                source.unlink()
                with self.assertRaises(FileNotFoundError):
                    cxn.render_home(root / "missing", mcp, cxn.MODEL, cxn.EFFORT, "SYSTEM")

    def test_auth_identity_does_not_record_gateway_in_chatgpt_mode(self) -> None:
        with mock.patch.dict(os.environ, {"CDB_CODEX_AUTH": "chatgpt"}):
            identity = cxn.observed(Path("/nonexistent"))
        self.assertEqual(identity["auth_path"], "chatgpt")
        self.assertNotIn("gateway", identity)
        with mock.patch.dict(os.environ, {"CDB_CODEX_AUTH": "gateway"}):
            identity = cxn.observed(Path("/nonexistent"))
        self.assertEqual(identity["auth_path"], "gateway")
        self.assertEqual(identity["gateway"], cxn.gateway())

    def test_invalid_auth_mode_is_rejected(self) -> None:
        with mock.patch.dict(os.environ, {"CDB_CODEX_AUTH": "invalid"}):
            with self.assertRaises(ValueError):
                cxn.config_toml(
                    {"command": "node", "args": [], "env": {}},
                    cxn.MODEL,
                    cxn.EFFORT,
                    "",
                    Path("/tmp/home"),
                )

    def test_config_uses_arm_b_launcher_and_switches_routes_off(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            env = cxn.render_home(
                tmp_path / "home", write_arm_b(tmp_path), cxn.MODEL, cxn.EFFORT, "SYSTEM"
            )
            cfg = tomllib.loads((Path(env["CODEX_HOME"]) / "config.toml").read_text("utf-8"))
        self.assertEqual(cfg["model"], cxn.MODEL)
        self.assertEqual(cfg["model_reasoning_effort"], cxn.EFFORT)
        self.assertEqual(cfg["approval_policy"], "on-request")  # elicitations reach the host
        self.assertEqual(cfg["sandbox_mode"], "read-only")
        self.assertEqual(cfg["developer_instructions"], "SYSTEM")
        for feature in (
            "shell_tool",
            "unified_exec",
            "browser_use",
            "in_app_browser",
            "multi_agent",
            "plugins",
        ):
            self.assertIs(cfg["features"][feature], False, feature)
        server = cfg["mcp_servers"]["cua_repl"]
        self.assertEqual(server["command"], ARM_B["mcpServers"]["codex-cu"]["command"])
        self.assertEqual(server["args"], ARM_B["mcpServers"]["codex-cu"]["args"])
        self.assertEqual(server["enabled_tools"], ["js", "js_reset"])
        self.assertEqual(server["env"]["CUA_REPL_ENABLED_SURFACES"], "computer")
        self.assertEqual(server["env"]["CODEX_HOME"], "/Users/lume/.codex")  # unchanged from arm B
        self.assertEqual(server["env"]["X"], "1")
        self.assertEqual(env["PATH"], cxn.CLOSED_PATH)

    def test_browser_surface_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                cxn.server_entry(write_arm_b(Path(tmp), "browser,computer"))

    def test_arm_is_registered(self) -> None:
        self.assertIn("cx-codex-cu", arms.ALL_ARMS)
        self.assertEqual(arms.NATIVE_CODEX_ARMS, cxn.ARMS)


def item(method: str, **fields: object) -> tuple[str, dict]:
    return method, {"item": fields, "threadId": "t", "turnId": "u"}


def usage(total_in: int, cached: int, out: int) -> tuple[str, dict]:
    return "thread/tokenUsage/updated", {
        "tokenUsage": {
            "total": {
                "inputTokens": total_in,
                "cachedInputTokens": cached,
                "outputTokens": out,
                "reasoningOutputTokens": 0,
            }
        }
    }


class CuaDriverUnderCodexTest(unittest.TestCase):
    """Amendment 23: Cua Driver's MCP server under the same Codex host."""

    def test_cua_server_has_no_tool_filter_and_no_surface_check(self) -> None:
        cua = {
            "mcpServers": {
                "cua": {
                    "type": "stdio",
                    "command": "/w/cua-driver",
                    "args": ["--socket", "/tmp/s", "mcp"],
                    "env": {"CUA_DRIVER_HOME": "/w/h"},
                }
            }
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "mcp.json"
            path.write_text(json.dumps(cua), "utf-8")
            env = cxn.render_home(
                Path(tmp) / "home", path, cxn.MODEL, cxn.EFFORT, "SYSTEM", "cx-cua-driver"
            )
            cfg = tomllib.loads((Path(env["CODEX_HOME"]) / "config.toml").read_text("utf-8"))
        server = cfg["mcp_servers"]["cua"]
        self.assertEqual(server["args"], ["--socket", "/tmp/s", "mcp"])
        self.assertNotIn("enabled_tools", server)
        self.assertEqual(server["env"], {"CUA_DRIVER_HOME": "/w/h"})
        self.assertNotIn("cua_repl", cfg["mcp_servers"])

    def test_registry(self) -> None:
        self.assertEqual(cxn.SOURCE_ARM["cx-cua-driver"], "cc-cua-driver-script")
        self.assertEqual(cxn.SOURCE_ARM["cx-claude-cu-helper"], "cc-claude-cu-helper")
        self.assertEqual(cxn.SERVER_OF["cx-claude-cu-helper"], "claude-cu-helper")
        self.assertIn("not an Anthropic-supported procedure", cxn.LABELS["cx-claude-cu-helper"])
        self.assertIn("cx-claude-cu-helper", arms.ALL_ARMS)
        self.assertIn("cx-cua-driver", arms.ALL_ARMS)
        self.assertEqual(cxn.MODEL, "gpt-6.1-sol")  # A23.1
        self.assertEqual(cxn.EFFORT, "low")

    def test_translator_names_the_cua_server(self) -> None:
        out = cxn.Translator("cua").feed("thread/started", {"thread": {"id": "t"}})
        self.assertEqual(out[0]["mcp_servers"], [{"name": "cua", "status": "connected"}])


class ChatGPTPreflightTest(unittest.TestCase):
    def test_metadata_only_probe_handles_notifications_pages_and_failures(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "auth.json"
            source.touch()
            home = root / "home"
            home.mkdir()
            (home / "auth.json").symlink_to(source)
            cli = root / "codex"
            requests = root / "requests"
            pid_file = root / "pid"
            for scenario in (
                "ok",
                "apiKey",
                "missing",
                "model",
                "rpc",
                "exit",
                "timeout",
                "malformed",
                "replace",
            ):
                cli.write_text(f"""#!{sys.executable}
import json, os, sys, time
from pathlib import Path
Path({str(pid_file)!r}).write_text(str(os.getpid()))
scenario = {scenario!r}
for line in sys.stdin:
    msg = json.loads(line)
    with open({str(requests)!r}, 'a') as log:
        log.write(line)
    if 'id' not in msg:
        continue
    method = msg['method']
    if scenario == 'exit':
        sys.exit(0)
    if scenario == 'malformed':
        print('[]', flush=True)
        continue
    if scenario == 'timeout':
        time.sleep(10)
    result = {{}}
    if method == 'account/read':
        assert msg['params']['refreshToken'] is False
        result = {{'account': None if scenario == 'missing' else {{'type': 'apiKey' if scenario == 'apiKey' else 'chatgpt', 'email': 'SECRET'}}}}
    if method == 'model/list':
        if not msg['params'].get('cursor'):
            result = {{'data': [], 'nextCursor': 'page2'}}
        else:
            result = {{'data': [{{'id': 'other' if scenario == 'model' else 'gpt-6.1-sol'}}], 'nextCursor': None}}
        if scenario == 'replace':
            link = Path({str(home / "auth.json")!r})
            link.unlink()
            link.touch()
    print(json.dumps({{'method': 'notification', 'params': {{'private': 'SECRET'}}}}), flush=True)
    reply = {{'id': msg['id'], 'result': result}}
    if scenario == 'rpc':
        reply = {{'id': msg['id'], 'error': {{'message': 'SECRET'}}}}
    print(json.dumps(reply), flush=True)
""")
                cli.chmod(0o755)
                requests.write_text("")
                with mock.patch.object(cxn, "CHATGPT_CHECK_TIMEOUT", 0.3):
                    checks = cxn.chatgpt_checks(cli, {"CODEX_HOME": str(home)}, cxn.MODEL)
                self.assertEqual(
                    all(status == "pass" for _, status, _ in checks), scenario == "ok", scenario
                )
                self.assertNotIn("SECRET", str(checks))
                with self.assertRaises(ProcessLookupError):
                    os.kill(int(pid_file.read_text()), 0)
                methods = [json.loads(line)["method"] for line in requests.read_text().splitlines()]
                self.assertTrue(
                    set(methods) <= {"initialize", "initialized", "account/read", "model/list"}
                )
                if scenario == "ok":
                    self.assertEqual(
                        methods,
                        ["initialize", "initialized", "account/read", "model/list", "model/list"],
                    )


class TranslatorTest(unittest.TestCase):
    def run_events(self, events: list[tuple[str, dict]]) -> tuple[list[dict], cxn.Translator]:
        tr = cxn.Translator()
        out: list[dict] = []
        for method, params in events:
            out += tr.feed(method, params)
        out.append(tr.result(timed_out=False, capped=False, wall_ms=1000))
        return out, tr

    def test_mcp_calls_become_tool_uses_and_results(self) -> None:
        call = dict(
            id="exec-1",
            type="mcpToolCall",
            server="cua_repl",
            tool="js",
            arguments={"code": "await app.click(1)"},
        )
        out, tr = self.run_events(
            [
                ("thread/started", {"thread": {"id": "t"}}),
                usage(50, 0, 3),
                item("item/started", **call, status="inProgress"),
                item("item/completed", **call, status="completed", error=None),
                usage(100, 40, 7),
                item(
                    "item/completed", id="m", type="agentMessage", text="DONE", phase="final_answer"
                ),
                ("turn/completed", {"turn": {"status": "completed", "error": None}}),
            ]
        )
        summary = claude_events.summarize(out)
        self.assertEqual(summary["tool_calls"]["total"], 1)
        self.assertEqual(summary["tool_calls"]["by_name"], {"mcp__cua_repl__js": 1})
        self.assertEqual(summary["tool_calls"]["by_class"], {"click": 1})
        self.assertEqual(summary["final_text"], "DONE")
        self.assertEqual(
            summary["tokens"], {"input": 60, "output": 7, "cache_read": 40, "cache_write": 0}
        )
        self.assertEqual(summary["result"]["num_turns"], 2)
        self.assertFalse(summary["result"]["is_error"])
        self.assertTrue(summary["has_init"])
        self.assertEqual(v2.scan_events(out), ([], []))

    def test_non_gui_items_are_violations(self) -> None:
        out, tr = self.run_events(
            [
                item("item/started", id="a", type="commandExecution", command="ls"),
                item("item/completed", id="b", type="fileChange", changes=[{"path": "x"}]),
                item("item/started", id="c", type="collabAgentToolCall", tool="spawn_agent"),
                item("item/completed", id="d", type="imageView", path="/tmp/x.png"),
                item("item/completed", id="e", type="reasoning"),
            ]
        )
        hard, _ = v2.scan_events(out)
        self.assertEqual(hard, ["builtin:Agent", "builtin:Bash", "builtin:Edit", "builtin:Task"])
        self.assertEqual(
            tr.non_gui, ["commandExecution", "fileChange", "collabAgentToolCall", "imageView"]
        )

    def test_code_in_js_is_scanned_like_arm_b(self) -> None:
        call = dict(
            id="i",
            type="mcpToolCall",
            server="cua_repl",
            tool="js",
            arguments={"code": "const cp = await import('node:child_process')"},
        )
        out, _ = self.run_events([item("item/started", **call)])
        hard, _ = v2.scan_events(out)
        self.assertTrue(any("child_process" in h for h in hard), hard)

    def test_failed_call_and_errors(self) -> None:
        call = dict(
            id="i", type="mcpToolCall", server="cua_repl", tool="js", arguments={"code": "x"}
        )
        out, _ = self.run_events(
            [
                ("thread/started", {"thread": {"id": "t"}}),
                item("item/completed", **call, status="failed", error={"message": "boom"}),
                ("error", {"error": {"message": "unexpected status 429 Too Many Requests"}}),
                ("turn/completed", {"turn": {"status": "failed", "error": {"message": "429"}}}),
            ]
        )
        summary = claude_events.summarize(out)
        self.assertEqual(summary["tool_calls"]["failed"], 1)
        self.assertTrue(summary["result"]["is_error"])
        self.assertEqual(summary["result"]["api_error_status"], 429)
        self.assertEqual(claude_events.classify_failure(summary)["kind"], "rate_limit")

    def test_turn_cap_result(self) -> None:
        result = cxn.Translator().result(timed_out=False, capped=True, wall_ms=1)
        self.assertEqual(result["subtype"], "error_max_turns")
        self.assertTrue(result["is_error"])


class GuardTest(unittest.TestCase):
    def test_guard_reads_the_translated_stream_live(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            stream = Path(tmp) / "s.tsv"
            lines = cxn.Translator().feed(
                *item("item/started", id="a", type="commandExecution", command="osascript -e 1")
            )
            stream.write_text("".join(f"1.0\t{json.dumps(e)}\n" for e in lines), "utf-8")
            guard = v2.GuiOnlyGuard(stream, lambda: "com.example.app", pgrep=lambda _n: False)
            guard.sample()
            self.assertTrue(guard.stop.is_set())
            self.assertEqual(guard.violations[0]["detail"], "builtin:Bash")


if __name__ == "__main__":
    unittest.main()
