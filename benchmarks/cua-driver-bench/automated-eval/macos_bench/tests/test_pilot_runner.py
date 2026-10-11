"""Unit tests for the pilot runner pieces that do not need a GUI or a model."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import arms  # noqa: E402
import codex_events  # noqa: E402


def _stamp(events: list[tuple[float, dict]]) -> Path:
    handle = tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False)
    for stamp, event in events:
        handle.write(f"{stamp}\t{json.dumps(event)}\n")
    handle.close()
    return Path(handle.name)


def _mcp(item_id: str, server: str, tool: str, arguments: dict, status: str = "completed") -> dict:
    return {
        "id": item_id,
        "type": "mcp_tool_call",
        "server": server,
        "tool": tool,
        "arguments": arguments,
        "status": status,
    }


class CodexEventsTest(unittest.TestCase):
    def test_classes_latency_and_tokens(self) -> None:
        events = [
            (1000.0, {"type": "item.started", "item": _mcp("a", "cua", "click", {})}),
            (1250.0, {"type": "item.completed", "item": _mcp("a", "cua", "click", {})}),
            (
                1300.0,
                {
                    "type": "item.started",
                    "item": {
                        "id": "b",
                        "type": "command_execution",
                        "command": "cua-driver get_window_state '{}'",
                    },
                },
            ),
            (
                1400.0,
                {
                    "type": "item.completed",
                    "item": {
                        "id": "b",
                        "type": "command_execution",
                        "command": "cua-driver get_window_state '{}'",
                    },
                },
            ),
            (
                1500.0,
                {
                    "type": "item.completed",
                    "item": {"id": "c", "type": "command_execution", "command": "ls ../evaluator"},
                },
            ),
            (
                1600.0,
                {
                    "type": "item.completed",
                    "item": _mcp(
                        "d",
                        "node_repl",
                        "js",
                        {
                            "code": "await sky.click({app:'x',x:1,y:2}); await sky.type_text({app:'x',text:'a'})"
                        },
                    ),
                },
            ),
            (
                1650.0,
                {"type": "item.completed", "item": _mcp("e", "codex", "list_mcp_resources", {})},
            ),
            (
                1700.0,
                {
                    "type": "item.completed",
                    "item": {
                        "id": "m",
                        "type": "agent_message",
                        "text": "Would you like me to continue?",
                    },
                },
            ),
            (
                1800.0,
                {
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 10,
                        "cached_input_tokens": 4,
                        "output_tokens": 3,
                        "reasoning_output_tokens": 1,
                    },
                },
            ),
        ]
        path = _stamp(events)
        summary = codex_events.summarize(codex_events.read_events(path))
        self.assertTrue(summary["turn_completed"])
        self.assertEqual(summary["tool_calls"]["mcp"], 2)
        self.assertEqual(summary["tool_calls"]["cua_cli_via_shell"], 1)
        self.assertEqual(summary["tool_calls"]["by_class"]["click"], 2)
        self.assertEqual(summary["tool_calls"]["by_class"]["type"], 1)
        self.assertEqual(summary["tool_calls"]["by_class"]["observe"], 1)
        self.assertEqual(summary["shell_commands"], 1)
        self.assertEqual(summary["steps"], 4)
        self.assertTrue(summary["evaluator_read_suspected"])
        self.assertTrue(summary["confirmation_requested"])
        self.assertEqual(
            summary["tokens"], {"input": 10, "cached_input": 4, "output": 3, "reasoning": 1}
        )
        self.assertEqual(summary["action_latency_ms"]["click"]["n"], 2)

    def test_error_event_marks_infrastructure(self) -> None:
        events = [(1.0, {"type": "error", "message": "429 Too Many Requests: rate limit"})]
        summary = codex_events.summarize(codex_events.read_events(_stamp(events)))
        self.assertTrue(summary["infra_error_suspected"])
        self.assertFalse(summary["turn_completed"])

    def test_rollout_usage_takes_last_cumulative_count(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            sessions = Path(tmp) / "sessions" / "2026" / "10" / "05"
            sessions.mkdir(parents=True)
            lines = []
            for total in (100, 250):
                lines.append(
                    json.dumps(
                        {
                            "payload": {
                                "type": "token_count",
                                "info": {
                                    "total_token_usage": {
                                        "input_tokens": total,
                                        "cached_input_tokens": total // 2,
                                        "output_tokens": 7,
                                        "reasoning_output_tokens": 2,
                                    }
                                },
                            }
                        }
                    )
                )
            (sessions / "rollout-x.jsonl").write_text("\n".join(lines) + "\n", "utf-8")
            self.assertEqual(
                codex_events.rollout_usage(Path(tmp)),
                {"input": 250, "cached_input": 125, "output": 7, "reasoning": 2},
            )
            self.assertIsNone(codex_events.rollout_usage(Path(tmp) / "missing"))


_CODEX_NODE_REPL_DOC = Path(
    "/Applications/ChatGPT.app/Contents/Resources/plugins/openai-bundled/plugins/computer-use/.codex-plugin/computer-use-node-repl.md"
)


class ArmsTest(unittest.TestCase):
    @unittest.skipUnless(
        _CODEX_NODE_REPL_DOC.exists(), "needs the ChatGPT app's bundled computer-use plugin"
    )
    def test_arm_homes_differ_only_in_tool_layer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cua = Path(tmp) / "cua"
            native = Path(tmp) / "native"
            arms.render_home("cua-driver-mcp", cua, "m", "high")
            arms.render_home("codex-native-cu", native, "m", "high")
            common = arms.common_config("m", "high")
            for home in (cua, native):
                text = (home / ".codex" / "config.toml").read_text("utf-8")
                self.assertTrue(text.startswith(common))
                self.assertIn('default_tools_approval_mode = "approve"', text)
            self.assertIn("[mcp_servers.cua]", (cua / ".codex/config.toml").read_text("utf-8"))
            self.assertIn(
                "[mcp_servers.node_repl]", (native / ".codex/config.toml").read_text("utf-8")
            )

    def test_exec_flags_are_identical_across_arms(self) -> None:
        argv = arms.codex_argv(Path("/w"), "m")
        self.assertIn("workspace-write", argv)
        self.assertNotIn("--dangerously-bypass-approvals-and-sandbox", argv)

    def test_preamble_forbids_confirmation_requests(self) -> None:
        self.assertIn("do not ask for confirmation", arms.PREAMBLE)


class ScheduleTest(unittest.TestCase):
    def test_schedule_is_deterministic_and_balanced(self) -> None:
        sys.path.insert(0, str(HERE.parent))
        import run_pilot

        first = run_pilot.build_schedule(["A", "B", "C"], ["x", "y"], 3, 7)
        second = run_pilot.build_schedule(["A", "B", "C"], ["x", "y"], 3, 7)
        self.assertEqual(first, second)
        self.assertEqual(len(first), 18)
        for task in "ABC":
            for run in range(3):
                arms_seen = [e["arm"] for e in first if e["task"] == task and e["run_index"] == run]
                self.assertCountEqual(arms_seen, ["x", "y"])
        self.assertEqual([e["order_index"] for e in first], list(range(18)))

    def test_probe_seed_is_stable_and_shared(self) -> None:
        import run_pilot

        self.assertEqual(
            run_pilot.probe_seed("PROBE-FORMS", 0), run_pilot.probe_seed("PROBE-FORMS", 0)
        )
        self.assertNotEqual(
            run_pilot.probe_seed("PROBE-FORMS", 0), run_pilot.probe_seed("PROBE-FORMS", 1)
        )


if __name__ == "__main__":
    unittest.main()
