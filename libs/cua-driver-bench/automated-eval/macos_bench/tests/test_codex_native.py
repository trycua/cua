# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Amendment 20 (CUA-1347): the native Codex arm (Codex CLI + gpt-6-astra driving Codex computer use)."""

from __future__ import annotations

import contextlib
import json
import shutil
import os
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

import pytest

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import arms  # noqa: E402
import bench_v2 as v2  # noqa: E402
import claude_events  # noqa: E402
import codex_native as cxn  # noqa: E402


@pytest.mark.parametrize("output", ["OUTPUT" * 1000, "short output"], ids=["truncated", "short"])
def test_browser_cli_translation_and_rejections(output):
    import browser_use_cli as bu

    assert bu.ARM in arms.ALL_ARMS and bu.ARM in cxn.ARMS
    assert bu.ARM in cxn.LABELS and bu.ARM not in cxn.MCP_ARMS
    command = "browser-use open 'https://example.test/a b'"
    wrapped = "/bin/zsh -lc " + __import__("shlex").quote(command)
    for text in (command, wrapped, f"{bu.EXECUTABLE} open 'https://example.test/a b'"):
        tr = cxn.Translator(command_matcher=bu.match)
        item = {"id": "cli1", "type": "commandExecution", "command": text}
        events = tr.feed("item/started", {"item": item})
        assert tr.feed("item/started", {"item": item}) == []
        events += tr.feed(
            "item/completed",
            {
                "item": {
                    **item,
                    "status": "completed",
                    "exitCode": 0,
                    "aggregatedOutput": output,
                }
            },
        )
        uses = [use for event in events for use in v2.tool_uses(event)]
        assert uses == [
            (
                "mcp__browser_use_cli__invoke",
                {"argv": [str(bu.EXECUTABLE), "open", "https://example.test/a b"], "python": None},
            )
        ]
        result = events[-1]["message"]["content"][0]
        assert result["tool_use_id"] == "cli1" and not result["is_error"]
        assert result["content"] == output[:2000]
        assert tr.non_gui == []
    for text in (
        "browser-use open x; curl x",
        "browser-use open $(whoami)",
        "browser-use open `id`",
        "browser-use open x | cat",
        "browser-use open x > out",
        "browser-use open $HOME",
        "browser-use open x\necho y",
        "/tmp/browser-use open x",
        "curl x",
        "wget x",
        "browser-use python <<'PY'\nprint(1)\nPY",
        "browser-use open 'unterminated",
    ):
        assert bu.match(text) is None
        events = cxn.Translator(command_matcher=bu.match).feed(
            "item/started", {"item": {"id": "bad", "type": "commandExecution", "command": text}}
        )
        name, tool_input = v2.tool_uses(events[0])[0]
        assert v2.scan_tool_use(name, tool_input)[0] == ["builtin:Bash"]


def test_browser_cli_python_and_argv_remain_scanned():
    import browser_use_cli as bu
    import run_bench as rb

    for argv, python in (
        (("browser-use", "subprocess"), None),
        (("browser-use", "python"), "subprocess evaluator-secret"),
    ):
        tr = cxn.Translator(command_matcher=lambda _: bu.Invocation(argv, python))
        events = tr.feed(
            "item/completed",
            {
                "item": {
                    "id": "py",
                    "type": "commandExecution",
                    "command": "injected",
                    "status": "completed",
                    "exitCode": 1,
                    "aggregatedOutput": "failure",
                }
            },
        )
        name, tool_input = v2.tool_uses(events[0])[0]
        assert v2.scan_tool_use(name, tool_input)[0]
        assert events[-1]["message"]["content"][0]["is_error"]
        if python:
            assert rb.evaluator_peeks(events, ["evaluator-secret"]) == [
                "mcp__browser_use_cli__invoke:evaluator-secret"
            ]


def test_browser_cli_offline_installer(tmp_path):
    import hashlib
    import pytest
    import browser_use_cli as bu
    from tools import install_browser_use as installer

    pins = json.loads((HERE / "pins.json").read_text())["browser_use_cli"]
    wheel = tmp_path / "browser_use-0.13.11-py3-none-any.whl"
    wheel.write_bytes(b"test wheel")
    with mock.patch.object(installer.subprocess, "run") as run:
        with pytest.raises(ValueError):
            installer.install(wheel)
        run.assert_not_called()
        installer.install(wheel, expected_sha=hashlib.sha256(wheel.read_bytes()).hexdigest())
        cmd = run.call_args.args[0]
        assert cmd == ["uv", "tool", "install", "--python", "3.12", str(wheel)]
        assert run.call_args.kwargs["env"]["UV_TOOL_BIN_DIR"] == str(bu.EXECUTABLE.parent)
        assert pins["version"] == "0.13.11"


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
            for arm in cxn.MCP_ARMS:
                configs = {}
                texts = {}
                for mode in ("gateway", "chatgpt"):
                    with mock.patch.dict(
                        os.environ, {"CDB_CODEX_AUTH": mode, "HOME": tmp}, clear=True
                    ):
                        env = cxn.render_home(
                            root / f"{arm}-{mode}", mcp, cxn.MODEL, cxn.EFFORT, "SYSTEM", arm
                        )
                        home = Path(env["CODEX_HOME"])
                        texts[mode] = (home / "config.toml").read_text()
                        configs[mode] = tomllib.loads(texts[mode])
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
                expected = texts["gateway"].replace(
                    'model_provider = "litellm"', 'cli_auth_credentials_store = "file"'
                )
                start = expected.index("[model_providers.litellm]")
                end = expected.index("\n\n", start)
                self.assertEqual(texts["chatgpt"], expected[:start] + expected[end + 1 :])
                gateway = configs["gateway"]
                self.assertEqual(gateway.pop("model_provider"), "litellm")
                self.assertIn("litellm", gateway.pop("model_providers"))
                self.assertNotIn("cli_auth_credentials_store", gateway)
                self.assertEqual(configs["chatgpt"].pop("cli_auth_credentials_store"), "file")
                self.assertEqual(gateway, configs["chatgpt"])
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
        self.assertEqual(identity["codex_auth"], "chatgpt")
        self.assertNotIn("gateway", identity)
        with mock.patch.dict(os.environ, {"CDB_CODEX_AUTH": "gateway"}):
            identity = cxn.observed(Path("/nonexistent"))
        self.assertEqual(identity["codex_auth"], "gateway")
        self.assertEqual(identity["gateway"], cxn.gateway())

    def test_credentials_are_never_read_copied_or_stat_followed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "shared-auth.json"
            source.write_text("FAKE_REFRESH_TOKEN")
            mcp = write_arm_b(root)
            skill = root / "skill"
            skill.mkdir()
            (skill / "SKILL.md").write_text("skill")

            def protect(function):
                def guarded(path, *args, **kwargs):
                    if Path(path) == source and kwargs.get("follow_symlinks", True):
                        raise AssertionError("credential content accessed")
                    return function(path, *args, **kwargs)

                return guarded

            with contextlib.ExitStack() as stack:
                stack.enter_context(
                    mock.patch.dict(
                        os.environ,
                        {
                            "CDB_CODEX_AUTH": "chatgpt",
                            "CDB_CODEX_AUTH_FILE": str(source),
                            "LITELLM_MASTER_KEY": "FAKE_GATEWAY_KEY",
                            "CDB_PILOT_PASS_GATEWAY_KEY": "1",
                        },
                    )
                )
                stack.enter_context(mock.patch.object(arms, "CUA_DRIVER_SKILL", skill))
                for owner, name in (
                    (__import__("builtins"), "open"),
                    (Path, "read_text"),
                    (Path, "read_bytes"),
                    (Path, "stat"),
                    (os, "open"),
                    *(
                        (shutil, name)
                        for name in (
                            "copy",
                            "copy2",
                            "copyfile",
                            "copytree",
                            "copymode",
                            "copystat",
                        )
                    ),
                ):
                    stack.enter_context(
                        mock.patch.object(owner, name, protect(getattr(owner, name)))
                    )
                for env in (
                    cxn.render_home(root / "native", mcp, cxn.MODEL, cxn.EFFORT, "SYSTEM"),
                    arms.render_home("codex-cua-driver", root / "fallback", "m", "high"),
                ):
                    self.assertNotIn("LITELLM_MASTER_KEY", env)
                    self.assertEqual((Path(env["CODEX_HOME"]) / "auth.json").readlink(), source)
                identity = json.dumps(cxn.observed(Path("/nonexistent")))
                for secret in (
                    str(source),
                    "FAKE_REFRESH_TOKEN",
                    "FAKE_GATEWAY_KEY",
                    cxn.gateway(),
                ):
                    self.assertNotIn(secret, identity)

    def test_gateway_config_matches_original_text(self) -> None:
        with mock.patch.dict(
            os.environ,
            {"CDB_CODEX_AUTH": "gateway", "CDB_CODEX_GATEWAY": "http://127.0.0.1:4000/v1"},
        ):
            text = cxn.config_toml(
                {"command": "node", "args": [], "env": {}},
                "gpt-6.1-sol",
                "low",
                "SYSTEM",
                Path("/trial"),
            )
        self.assertEqual(
            text,
            """model = "gpt-6.1-sol"
model_provider = "litellm"
model_reasoning_effort = "low"
approval_policy = "on-request"
sandbox_mode = "read-only"
web_search = "disabled"
mcp_optional_startup_grace_ms = 30000
developer_instructions = "SYSTEM"

[features]
shell_tool = false
unified_exec = false
apps = false
browser_use = false
browser_use_external = false
in_app_browser = false
image_generation = false
multi_agent = false
multi_agent_v2 = false
goals = false
view_image = false
plugins = false
tool_suggest = false
skill_search = false

[model_providers.litellm]
name = "LiteLLM"
base_url = "http://127.0.0.1:4000/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 2
stream_max_retries = 2
stream_idle_timeout_ms = 120000

[mcp_servers.cua_repl]
command = "node"
args = []
startup_timeout_sec = 120
default_tools_approval_mode = "approve"
enabled_tools = ["js", "js_reset"]

[mcp_servers.cua_repl.env]
""",
        )

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
            result = {{'data': [{{'model': 'other' if scenario == 'model' else 'gpt-6.1-sol', 'id': 'catalog-id'}}], 'nextCursor': None}}
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
    def test_completed_mcp_is_error_becomes_setup_failure(self) -> None:
        tr = cxn.Translator()
        events = tr.feed("thread/started", {"thread": {"id": "t"}})
        events += tr.feed(
            "item/completed",
            {
                "item": {
                    "id": "tool",
                    "type": "mcpToolCall",
                    "server": "cua",
                    "tool": "screenshot",
                    "status": "completed",
                    "result": {
                        "isError": True,
                        "content": [
                            {"type": "image", "data": "SECRET"},
                            {
                                "type": "text",
                                "text": "screen_recording_permission_denied" + "!" * 500,
                            },
                        ],
                    },
                }
            },
        )
        summary = claude_events.summarize(events)
        self.assertEqual(claude_events.classify_failure(summary)["kind"], "tool_setup")
        block = events[-1]["message"]["content"][0]
        self.assertTrue(block["is_error"])
        self.assertLessEqual(len(block["content"]), 300)
        self.assertNotIn("SECRET", json.dumps(events))

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
