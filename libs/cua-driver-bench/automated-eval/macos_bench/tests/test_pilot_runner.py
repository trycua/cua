# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Unit tests for the pilot runner pieces that do not need a GUI or a model."""

from __future__ import annotations

import argparse
import json
import os
import tomllib
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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


class ArmsTest(unittest.TestCase):
    def test_arm_homes_differ_only_in_tool_layer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in (
                "skills/computer-use/SKILL.md", ".codex-plugin/computer-use-node-repl.md"
            ):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture")
            source = root / ".codex/auth.json"
            source.parent.mkdir()
            source.write_bytes(b"private")
            source.chmod(0o000)
            before = source.stat().st_mtime_ns
            with mock.patch.multiple(
                arms,
                CODEX_APP_RESOURCES=root,
                CODEX_CU_PLUGIN=root,
                CUA_DRIVER_SKILL=root / "skills/computer-use",
            ):
                for arm in arms.ARMS:
                    configs = {}
                    for mode in ("gateway", "chatgpt"):
                        home = root / f"{arm}-{mode}"
                        with mock.patch.dict(
                            os.environ,
                            {
                                "CDB_CODEX_AUTH": mode,
                                "CDB_CODEX_AUTH_FILE": str(source),
                                "LITELLM_MASTER_KEY": "secret",
                                "CDB_PILOT_PASS_GATEWAY_KEY": "1",
                            },
                        ):
                            env = arms.render_home(arm, home, "m", "high")
                            text = (home / ".codex/config.toml").read_text()
                            self.assertTrue(text.startswith(arms.common_config("m", "high")))
                            self.assertEqual("LITELLM_MASTER_KEY" in env, mode == "gateway")
                            link = home / ".codex/auth.json"
                            self.assertEqual(link.is_symlink(), mode == "chatgpt")
                            if mode == "chatgpt":
                                self.assertEqual(os.readlink(link), str(source))
                            configs[mode] = tomllib.loads(text)
                    gateway = configs["gateway"]
                    self.assertEqual(gateway.pop("model_provider"), "litellm")
                    gateway.pop("model_providers")
                    self.assertEqual(gateway, configs["chatgpt"])
                    self.assertEqual(gateway["cli_auth_credentials_store"], "file")
                    if arm == "codex-native-cu":
                        self.assertIn("notify", gateway)
                with mock.patch.dict(
                    os.environ, {"CDB_CODEX_AUTH": "chatgpt", "HOME": tmp}, clear=True
                ):
                    env = arms.render_home("cua-driver-mcp", root / "default", "m", "high")
                    link = Path(env["CODEX_HOME"]) / "auth.json"
                    self.assertTrue(link.is_symlink())
                    self.assertEqual(os.readlink(link), str(source))
                self.assertEqual(source.stat().st_mtime_ns, before)
                source.chmod(0o600)
                self.assertEqual(source.read_bytes(), b"private")
                source.unlink()
                with mock.patch.dict(
                    os.environ, {"CDB_CODEX_AUTH": "chatgpt", "CDB_CODEX_AUTH_FILE": str(source)}
                ):
                    with self.assertRaises(FileNotFoundError):
                        arms.render_home("cua-driver-mcp", root / "missing", "m", "high")

    def test_exec_flags_are_identical_across_arms(self) -> None:
        argv = arms.codex_argv(Path("/w"), "m")
        self.assertIn("workspace-write", argv)
        self.assertNotIn("--dangerously-bypass-approvals-and-sandbox", argv)

    def test_preamble_forbids_confirmation_requests(self) -> None:
        self.assertIn("do not ask for confirmation", arms.PREAMBLE)


class ScheduleTest(unittest.TestCase):
    def test_pilot_rows_and_manifest_record_auth_mode(self) -> None:
        import run_pilot

        args = argparse.Namespace(
            model="m", effort="high", idle_min=0, max_idle_wait_h=1, no_sentinel=True
        )
        task = argparse.Namespace(id="T", group="bench", tags=[], timeout_s=60)
        for mode in ("gateway", "chatgpt"):
            with (
                tempfile.TemporaryDirectory() as tmp,
                mock.patch.dict(os.environ, {"CDB_CODEX_AUTH": mode}),
                mock.patch.object(run_pilot, "wait_for_idle", return_value=1000),
                mock.patch.object(run_pilot, "Sentinel") as sentinel,
                mock.patch.object(
                    run_pilot, "prepare_bench", side_effect=RuntimeError("unavailable fixture")
                ),
                mock.patch.object(run_pilot, "sweep_processes"),
                mock.patch.object(run_pilot.subprocess, "run") as command,
            ):
                sentinel.return_value.enabled = False
                sentinel.return_value.summary.return_value = {}
                command.return_value.stdout = "version"
                row = run_pilot.run_trial(args, None, task, "codex-native-cu", 1, 0, Path(tmp), None)
                manifest = run_pilot.environment_manifest(args)
                self.assertEqual(row["codex_auth_path"], mode)
                self.assertEqual(manifest["codex_auth_path"], mode)
                saved = json.loads(next((Path(tmp) / "trials").glob("*/trial.json")).read_text())
                self.assertEqual(saved["codex_auth_path"], mode)

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
