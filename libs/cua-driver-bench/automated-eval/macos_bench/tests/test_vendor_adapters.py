# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Adapters for agent-desktop, the Browser Use harnesses and the Codex computer-use filter (no GUI, no model)."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOLS = HERE.parent / "tools"
for sub in ("agent_desktop", "browser_use_harness", "codex_cu"):
    sys.path.insert(0, str(TOOLS / sub))
sys.path.insert(0, str(HERE.parent))

import ad_mcp  # noqa: E402
import arms  # noqa: E402
import bu_mcp  # noqa: E402
import claude_arms as ca  # noqa: E402
import claude_events as ce  # noqa: E402
import codex_cu_filter as cf  # noqa: E402


def rpc(script: Path, env: dict[str, str], *messages: dict) -> list[dict]:
    data = "".join(json.dumps(m) + "\n" for m in messages)
    out = subprocess.run(
        [sys.executable, "-I", "-B", str(script)],
        input=data,
        capture_output=True,
        text=True,
        env={"PATH": os.environ.get("PATH", ""), **env},
        timeout=60,
    )
    return [json.loads(line) for line in out.stdout.splitlines() if line.strip()]


class AgentDesktopAdapterTest(unittest.TestCase):
    def test_every_tool_maps_to_a_cli_command(self) -> None:
        names = {t["name"] for t in ad_mcp.tools()}
        self.assertEqual(names, set(ad_mcp.T))
        self.assertEqual(
            ad_mcp.argv_for("click", {"ref": "@s:e3"}),
            ["click", "@s:e3"],
        )
        self.assertEqual(
            ad_mcp.argv_for("snapshot", {"app": "TextEdit", "interactive_only": True}),
            ["snapshot", "--app", "TextEdit", "-i"],
        )

    def test_headed_is_a_model_choice_only_in_agent_mode(self) -> None:
        self.assertEqual(ad_mcp.argv_for("click", {"ref": "@s:e3", "headed": True})[0], "--headed")
        self.assertEqual(ad_mcp.argv_for("click", {"ref": "@s:e3"})[0], "click")
        self.assertEqual(
            ad_mcp.argv_for("hover", {"xy": "1,2"})[0], "--headed"
        )  # pointer-only tool

    def test_negative_values_keep_their_sign_as_one_argument(self) -> None:
        self.assertEqual(
            ad_mcp.argv_for("mouse_wheel", {"x": 1, "y": 2, "dy": "-5"}),
            ["--headed", "mouse-wheel", "--x", "1", "--y", "2", "--dy=-5"],
        )

    def test_server_round_trip_with_a_fake_binary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "agent-desktop"
            fake.write_text('#!/bin/sh\nprintf \'{"ok":true,"args":"%s"}\' "$*"\n')
            fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
            replies = rpc(
                TOOLS / "agent_desktop" / "ad_mcp.py",
                {"AD_BIN": str(fake)},
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                {
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": "list_apps", "arguments": {"app": "Finder"}},
                },
            )
        self.assertIn("agent-desktop", replies[0]["result"]["instructions"])
        self.assertGreater(len(replies[1]["result"]["tools"]), 30)
        self.assertIn("list-apps --app Finder", replies[2]["result"]["content"][0]["text"])
        self.assertFalse(replies[2]["result"]["isError"])


class MacosHarnessAdapterTest(unittest.TestCase):
    def test_only_vendor_commands_joined_by_and_are_accepted(self) -> None:
        steps = bu_mcp.parse(
            'macos-harness click --app "TextEdit" --element_index 12 && macos-harness get_app_state --app "TextEdit"'
        )
        self.assertEqual([s[0][0] for s in steps], ["click", "get_app_state"])
        self.assertEqual(steps[0][0][2], "TextEdit")

    def test_heredoc_goes_to_the_last_command_only(self) -> None:
        steps = bu_mcp.parse("macos-harness paste --app Notes <<'EOF'\nhello \"x\"\nEOF")
        self.assertEqual(steps[-1][1], 'hello "x"')

    def test_shell_syntax_and_vendor_admin_commands_are_refused(self) -> None:
        for command in (
            "macos-harness list_apps | cat",
            "macos-harness list_apps; ls",
            "macos-harness list_apps > /tmp/x",
            "macos-harness doctor",
            "macos-harness install",
            "ls",
        ):
            with self.assertRaises(ValueError, msg=command):
                bu_mcp.parse(command)

    def test_server_round_trip_with_a_fake_binary_and_screenshot_rule(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            shot = Path(tmp) / "s.jpeg"
            shot.write_bytes(b"\xff\xd8\xff\xd9")
            other = Path(tmp) / "other.jpeg"
            other.write_bytes(b"\xff\xd8\xff\xd9")
            fake = Path(tmp) / "macos-harness"
            fake.write_text(
                f'#!/bin/sh\nif [ "$1" = skill ]; then echo SKILLTEXT; exit 0; fi\n'
                f'echo "Screenshot (1x1 px): {shot}"\n'
            )
            fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
            replies = rpc(
                TOOLS / "browser_use_harness" / "bu_mcp.py",
                {"BU_BIN": str(fake)},
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
                {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "macos_harness",
                        "arguments": {"command": "macos-harness get_app_state --app Notes"},
                    },
                },
                {
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": "view_screenshot", "arguments": {"path": str(shot)}},
                },
                {
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {"name": "view_screenshot", "arguments": {"path": str(other)}},
                },
            )
        self.assertEqual(replies[0]["result"]["instructions"].strip(), "SKILLTEXT")
        self.assertFalse(replies[1]["result"]["isError"])
        self.assertEqual(replies[2]["result"]["content"][0]["type"], "image")
        self.assertTrue(replies[3]["result"]["isError"])  # a file the harness never printed


class WindowsHarnessAdapterTest(unittest.TestCase):
    def test_it_compiles_and_names_only_the_allowed_subcommands(self) -> None:
        source = (TOOLS / "browser_use_harness" / "wh_mcp.py").read_text("utf-8")
        compile(source, "wh_mcp.py", "exec")
        self.assertIn('ALLOWED = {"apps", "see", "state", "exec", "skill"}', source)
        self.assertIn('os.environ.get("WH_BIN")', source)


class CodexFilterTest(unittest.TestCase):
    def setUp(self) -> None:
        self.f = cf.Filter({"Calculator"}, "/tmp/work")

    def test_tools_call_gets_sandbox_meta(self) -> None:
        line = json.dumps(
            {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "js"}}
        ).encode()
        out = json.loads(self.f.rewrite_up(line + b"\n"))
        meta = out["params"]["_meta"][cf.KEY]
        self.assertEqual(meta["permissionProfile"], {"type": "disabled"})
        self.assertTrue(meta["sandboxCwd"].startswith("file://"))
        other = b'{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n'
        self.assertEqual(self.f.rewrite_up(other), other)

    def test_approval_prompt_only_for_allowlisted_apps(self) -> None:
        def ask(text: str) -> dict:
            msg = {
                "jsonrpc": "2.0",
                "id": 7,
                "method": "elicitation/create",
                "params": {"message": text},
            }
            return json.loads(self.f.elicit_reply(json.dumps(msg).encode() + b"\n"))["result"]

        self.assertEqual(ask('Allow Computer Use to use "Calculator"?')["action"], "accept")
        self.assertEqual(ask('Allow Computer Use to use "Terminal"?')["action"], "decline")
        self.assertEqual(ask("Allow something else?")["action"], "decline")
        self.assertIsNone(self.f.elicit_reply(b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}\n'))

    def test_server_discover_is_answered_locally(self) -> None:
        reply = json.loads(
            self.f.discover_reply(b'{"jsonrpc":"2.0","id":3,"method":"server/discover"}\n')
        )
        self.assertEqual(reply["error"]["code"], -32601)
        self.assertIsNone(
            self.f.discover_reply(b'{"jsonrpc":"2.0","id":3,"method":"initialize"}\n')
        )

    def test_one_of_schema_is_flattened(self) -> None:
        tools = {
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [
                    {
                        "name": "js",
                        "inputSchema": {
                            "type": "object",
                            "oneOf": [
                                {"properties": {"code": {"type": "string"}}, "required": ["code"]},
                                {"properties": {"completion_summary": {"type": "string"}}},
                            ],
                        },
                    },
                    {"name": "js_reset", "inputSchema": {"type": "object", "properties": {}}},
                ]
            },
        }
        out = json.loads(self.f.flatten_down(json.dumps(tools).encode() + b"\n"))["result"]["tools"]
        self.assertEqual(set(out[0]["inputSchema"]["properties"]), {"code", "completion_summary"})
        self.assertNotIn("oneOf", out[0]["inputSchema"])
        self.assertNotIn("required", out[0]["inputSchema"])

    def test_windows_command_points_at_the_app_package(self) -> None:
        command, env = cf.windows_command("C:/apps/resources", home="C:/home/bench")
        self.assertTrue(command[0].endswith("node.exe"))
        self.assertTrue(command[1].endswith("cua-repl.mjs"))
        self.assertEqual(env["CUA_REPL_ENABLED_SURFACES"], "computer")


class ArmRegistrationTest(unittest.TestCase):
    def test_arms_are_named_claude_arms_but_not_default(self) -> None:
        for arm in ("cc-agent-desktop", "cc-browser-use"):
            self.assertIn(arm, arms.CLAUDE_ARMS)
            self.assertNotIn(arm, arms.DEFAULT_CLAUDE_ARMS)
            self.assertIn(arm, ca.ARM_DESCRIPTIONS)
            self.assertIn(arm, ca.ADAPTER_ARMS)

    def test_config_is_a_closed_stdio_entry_with_the_adapter(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            for arm, server in (
                ("cc-agent-desktop", "agentdesktop"),
                ("cc-browser-use", "browseruse"),
            ):
                path, name = ca.mcp_config_for(arm, Path(tmp))
                self.assertEqual(name, server)
                entry = json.loads(path.read_text("utf-8"))["mcpServers"][server]
                self.assertEqual(entry["args"][:2], ["-I", "-B"])
                self.assertTrue(Path(entry["args"][2]).is_file())
                self.assertEqual(entry["env"]["PATH"], ca.CLAUDE_PATH)

    def test_classes(self) -> None:
        self.assertEqual(ce.tool_class("mcp__agentdesktop__click"), "click")
        self.assertEqual(ce.tool_class("mcp__agentdesktop__snapshot"), "observe")
        self.assertEqual(
            ce.tool_class(
                "mcp__browseruse__macos_harness",
                {
                    "command": "macos-harness type_text --app A --text x && macos-harness get_app_state --app A"
                },
            ),
            "type",
        )


if __name__ == "__main__":
    unittest.main()
