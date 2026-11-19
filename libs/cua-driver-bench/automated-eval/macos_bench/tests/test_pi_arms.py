# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The pi-hosted Cua Driver arms: the tool surface they offer and how pi's events reach the guard and the row."""

from __future__ import annotations

import json
import os
import stat
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import arms  # noqa: E402
import bench_v2 as v2  # noqa: E402
import claude_arms as ca  # noqa: E402
import claude_events  # noqa: E402
import pi_arms as pia  # noqa: E402


def assistant(content, stop="toolUse", usage=None):
    return {
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": content,
            "provider": "openai-codex",
            "model": "gpt-6.1-terra",
            "stopReason": stop,
            "usage": usage or {"input": 10, "output": 2, "cacheRead": 5, "cost": {"total": 0.5}},
        },
    }


def call(call_id, name, arguments=None):
    return {"type": "toolCall", "id": call_id, "name": name, "arguments": arguments or {}}


SYSTEM = {
    "type": "message_end",
    "message": {
        "role": "system",
        "toolsAdded": [{"name": "read"}, {"name": "tool_search"}],
        "sections": {"skills": "<skill>\n<name>cua-driver</name>\n</skill>", "cwd": "x"},
    },
}


class PiArmsTest(unittest.TestCase):
    def test_each_arm_mirrors_one_driver_arm(self):
        self.assertEqual(set(pia.ARMS) & set(arms.ALL_ARMS), set(pia.ARMS))
        self.assertEqual(set(pia.SOURCE_ARM.values()), set(ca.CUA_ARMS))

    def test_argv_offers_read_tool_search_and_the_cua_server_only(self):
        argv = pia.argv("SYSTEM PROMPT", Path("/w/.claude/skills/cua-driver"), Path("/bin/pi"))
        self.assertEqual(argv[argv.index("--tools") + 1], "read,tool_search,mcp__cua__*")
        self.assertEqual(argv[argv.index("--system-prompt") + 1], "SYSTEM PROMPT")
        self.assertEqual(argv[argv.index("--skill") + 1], "/w/.claude/skills/cua-driver")
        for flag in ("--no-extensions", "--no-skills", "--no-context-files", "--no-session"):
            self.assertIn(flag, argv)
        loaded = [argv[i + 1] for i, a in enumerate(argv) if a == "-e"]
        self.assertEqual(loaded, ["builtin:mcp", "builtin:tool-search", str(pia.GATE)])
        surface = [pia.claude_name(t) for t in pia.TOOLS if not t.startswith("mcp__")]
        self.assertFalse(set(surface) & set(v2.FORBIDDEN_BUILTINS))

    def test_tools_outside_the_surface_are_forbidden_builtins(self):
        for name in ("bash", "edit", "write", "codemode", "grep", "mcp__other__run"):
            hard, _ = v2.scan_tool_use(pia.claude_name(name), {})
            self.assertTrue(hard, name)
        self.assertEqual(v2.scan_tool_use(pia.claude_name("read"), {"path": "x"}), ([], []))

    def test_agent_dir_copies_the_source_mcp_entry_deferred_and_links_auth(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            auth = root / "auth.json"
            auth.write_text("{}")
            source = root / "mcp.json"
            entry = {"type": "stdio", "command": "/d/cua-driver", "args": ["--socket", "/s", "mcp"]}
            source.write_text(json.dumps({"mcpServers": {"cua": entry}}))
            pi_root = root / "pi-root"
            with mock.patch.dict(os.environ, {"CDB_PI_AUTH_FILE": str(auth)}):
                env = pia.render_agent_dir(pi_root, source, root / "skill", 45)
            agent = Path(env["PI_CODING_AGENT_DIR"])
            config = json.loads((agent / "mcp.json").read_text())
            self.assertEqual(config["mcpServers"]["cua"], {**entry, "exposure": "deferred"})
            self.assertIs(config["autoEnableCodemode"], False)
            self.assertTrue(stat.S_ISLNK((agent / "auth.json").lstat().st_mode))
            self.assertNotEqual(env["HOME"], os.environ.get("HOME"))
            self.assertEqual(env["CDB_PI_READ_ALLOW"], str(root / "skill" / "SKILL.md"))
            self.assertEqual(env["CDB_PI_MAX_TURNS"], "45")
            pia.keep_config(pi_root, root / "trial" / "pi-agent")
            self.assertFalse(pi_root.exists())
            self.assertEqual(
                sorted(p.name for p in (root / "trial" / "pi-agent").iterdir()),
                ["mcp.json", "settings.json"],
            )

    def test_events_become_the_claude_shaped_stream(self):
        tr = pia.Translator(max_turns=45)
        events = tr.feed(SYSTEM)
        events += tr.feed(assistant([call("c1", "tool_search", {"query": "click"})]))
        events += tr.feed(
            {
                "type": "tool_execution_end",
                "toolCallId": "c1",
                "toolName": "tool_search",
                "result": {"content": [{"type": "text", "text": "Loaded 2 tools"}]},
            }
        )
        tr.feed(
            {
                "type": "message_end",
                "message": {"role": "system", "toolsAdded": [{"name": "mcp__cua__click"}]},
            }
        )
        events += tr.feed(assistant([call("c2", "mcp__cua__click", {"x": 1})]))
        events += tr.feed(
            {
                "type": "tool_execution_end",
                "toolCallId": "c2",
                "toolName": "mcp__cua__click",
                "isError": True,
                "result": {"content": [{"type": "text", "text": "no element"}]},
            }
        )
        events += tr.feed(assistant([{"type": "text", "text": "DONE"}], stop="stop"))
        events += tr.feed({"type": "agent_settled", "aborted": False})
        events += tr.finish(capped=False, wall_ms=1000)
        summary = claude_events.summarize(events)
        self.assertEqual(summary["init"]["tools_builtin"], ["Read", "ToolSearch"])
        self.assertEqual(summary["init"]["mcp_tools"], ["mcp__cua__click"])
        self.assertEqual(summary["init"]["skills"], ["cua-driver"])
        self.assertEqual(summary["init"]["mcp_servers"], [{"name": "cua", "status": "connected"}])
        self.assertEqual(summary["tool_calls"]["by_name"], {"ToolSearch": 1, "mcp__cua__click": 1})
        self.assertEqual(summary["tool_calls"]["failed_by_name"], {"mcp__cua__click": 1})
        self.assertEqual(summary["turns"], 3)
        self.assertEqual(summary["final_text"], "DONE")
        self.assertEqual(summary["result"]["subtype"], "success")
        self.assertEqual(
            summary["tokens"], {"input": 30, "output": 6, "cache_read": 15, "cache_write": 0}
        )
        self.assertAlmostEqual(summary["total_cost_usd"], 1.5)
        self.assertEqual(claude_events.classify_failure(summary, "", 0)["kind"], None)

    def test_turn_cap_is_reached_when_a_turn_past_the_cap_starts(self):
        tr = pia.Translator(max_turns=2)
        for n in (1, 2):
            tr.feed({"type": "turn_start"})
            tr.feed(assistant([call(f"c{n}", "mcp__cua__click")]))
            self.assertFalse(tr.capped.is_set())
        tr.feed({"type": "turn_start"})
        self.assertTrue(tr.capped.is_set())
        aborted = assistant([], stop="error")
        aborted["message"]["errorMessage"] = "This operation was aborted"
        self.assertEqual(tr.feed(aborted), [])
        result = tr.finish(capped=True, wall_ms=1)[-1]
        self.assertEqual(
            (result["subtype"], result["is_error"], result["num_turns"]),
            ("error_max_turns", True, 2),
        )

    def test_a_final_answer_on_the_last_allowed_turn_is_not_capped(self):
        tr = pia.Translator(max_turns=1)
        tr.feed({"type": "turn_start"})
        tr.feed(assistant([{"type": "text", "text": "DONE"}], stop="stop"))
        tr.feed({"type": "agent_settled"})
        self.assertFalse(tr.capped.is_set())
        self.assertEqual(tr.finish(capped=False, wall_ms=1)[-1]["subtype"], "success")

    def test_provider_error_is_classified_as_infrastructure(self):
        tr = pia.Translator(max_turns=45)
        message = assistant([], stop="error")
        message["message"]["errorMessage"] = "429 Too Many Requests: usage limit reached"
        events = tr.feed(message) + tr.feed({"type": "agent_settled"})
        summary = claude_events.summarize(events + tr.finish(capped=False, wall_ms=1, returncode=1))
        self.assertEqual(claude_events.classify_failure(summary, "", 0)["kind"], "rate_limit")

    def test_exit_without_settling_is_a_harness_failure_not_a_result(self):
        tr = pia.Translator(max_turns=45)
        events = tr.feed(SYSTEM) + tr.feed(assistant([call("c1", "mcp__cua__click")]))
        self.assertEqual(tr.finish(capped=False, wall_ms=1, returncode=1), [])
        summary = claude_events.summarize(events)
        self.assertEqual(
            claude_events.classify_failure(summary, "TypeError", 1)["kind"], "harness_crash"
        )

    def test_settled_with_nonzero_exit_is_a_harness_failure(self):
        tr = pia.Translator(max_turns=45)
        events = tr.feed(SYSTEM) + tr.feed(
            assistant([{"type": "text", "text": "DONE"}], stop="stop")
        )
        tr.feed({"type": "agent_settled"})
        self.assertEqual(tr.finish(capped=False, wall_ms=1, returncode=1), [])
        summary = claude_events.summarize(events)
        self.assertEqual(claude_events.classify_failure(summary, "", 1)["kind"], "harness_crash")

    def test_cap_is_read_after_pi_exits_right_after_the_over_cap_turn(self):
        lines = [
            assistant([call("c1", "mcp__cua__click")]),
            assistant([call("c2", "mcp__cua__click")]),
        ]
        script = textwrap.dedent(
            """\
            import json, sys
            sys.stdin.read()
            for event in json.load(open(sys.argv[1])):
                print(json.dumps({"type": "turn_start"}))
                print(json.dumps(event))
            print(json.dumps({"type": "turn_start"}), flush=True)
            """
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "fake.py").write_text(script)
            (root / "events.json").write_text(json.dumps(lines))
            result = pia.run_pi(
                [sys.executable, str(root / "fake.py"), str(root / "events.json")],
                {"PATH": os.environ["PATH"]},
                root,
                "brief",
                20,
                2,
                root / "raw.tsv",
                root / "stream.tsv",
                root / "stderr",
            )
            events = claude_events.read_events(root / "stream.tsv")
            self.assertTrue(result["capped"])
            self.assertEqual(events[-1]["subtype"], "error_max_turns")

    def test_no_turn_leaves_no_result_so_stderr_classifies_the_row(self):
        tr = pia.Translator(max_turns=45)
        self.assertEqual(tr.finish(capped=False, wall_ms=1), [])
        summary = claude_events.summarize([])
        failure = claude_events.classify_failure(summary, "Error: not logged in", 1)
        self.assertEqual(failure["kind"], "auth")

    def test_run_pi_streams_events_and_stops_at_the_turn_cap(self):
        lines = [SYSTEM] + [assistant([call(f"c{n}", "mcp__cua__click")]) for n in range(5)]
        script = textwrap.dedent(
            """\
            import json, sys, time
            sys.stdin.read()
            for event in json.load(open(sys.argv[1])):
                if event["message"]["role"] == "assistant":
                    print(json.dumps({"type": "turn_start"}))
                print(json.dumps(event), flush=True)
                time.sleep(0.05)
            time.sleep(30)
            """
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "fake.py").write_text(script)
            (root / "events.json").write_text(json.dumps(lines))
            result = pia.run_pi(
                [sys.executable, str(root / "fake.py"), str(root / "events.json")],
                {"PATH": os.environ["PATH"]},
                root,
                "brief",
                20,
                3,
                root / "raw.tsv",
                root / "stream.tsv",
                root / "stderr",
            )
            events = claude_events.read_events(root / "stream.tsv")
            self.assertTrue(result["capped"])
            self.assertFalse(result["timed_out"])
            self.assertLess(result["wall_s"], 10)
            self.assertEqual(result["turns"], 3)
            self.assertEqual(events[-1]["subtype"], "error_max_turns")
            self.assertEqual([n for n, _ in v2.tool_uses(events[0])], ["mcp__cua__click"])


if __name__ == "__main__":
    unittest.main()
