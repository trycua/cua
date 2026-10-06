"""Unit tests for the Claude Code benchmark runner logic: schedule, blocks, quota gate, ledger,
backoff, cutoff, stream-json parsing, failure classification, argv and environment scrubbing.
No GUI and no model calls."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import bench_core as core  # noqa: E402
import claude_arms as ca  # noqa: E402
import claude_driver  # noqa: E402
import claude_events as ce  # noqa: E402

TASKS = [f"MB-{i:02d}" for i in range(1, 13)]
ARMS = ["cc-cua-driver", "cc-codex-cu"]


class ScheduleTest(unittest.TestCase):
    def test_priority_order_mb_first_then_others(self) -> None:
        ids = ["CDB-S01", "MB-10", "MB-02", "MB-13", "MB-01"]
        self.assertEqual(core.order_tasks(ids), ["CDB-S01", "MB-01", "MB-02", "MB-10", "MB-13"])
        self.assertEqual(core.order_tasks(ids, explicit=True), ids)

    def test_first_arm_alternates_by_task_plus_run_parity(self) -> None:
        for pos in range(4):
            for run in range(5):
                order = core.arm_order(pos, run, ARMS)
                expected_first = ARMS[0] if (pos + run) % 2 == 0 else ARMS[1]
                self.assertEqual(order[0], expected_first)
                self.assertEqual(sorted(order), sorted(ARMS))

    def test_blocks_are_task_major_with_three_pairs_then_two(self) -> None:
        blocks = core.build_blocks(TASKS, ARMS, 3, 2, 20261005)
        phase1 = [b for b in blocks if b["phase"] == 1]
        phase2 = [b for b in blocks if b["phase"] == 2]
        self.assertEqual([b["task"] for b in phase1], TASKS)
        self.assertEqual([b["task"] for b in phase2], TASKS)
        self.assertEqual(blocks[: len(phase1)], phase1)  # all of phase 1 before any of phase 2
        for b in phase1:
            self.assertEqual(len(b["entries"]), 6)
            self.assertEqual(b["runs"], [0, 1, 2])
        for b in phase2:
            self.assertEqual(len(b["entries"]), 4)
            self.assertEqual(b["runs"], [3, 4])
        entries = core.flat_entries(blocks)
        self.assertEqual(len(entries), 12 * 6 + 12 * 4)
        self.assertEqual([e["order_index"] for e in entries], list(range(len(entries))))
        self.assertEqual(len({e["trial_id"] for e in entries}), len(entries))

    def test_pairs_are_back_to_back_and_balanced(self) -> None:
        entries = core.flat_entries(core.build_blocks(TASKS, ARMS, 3, 2, 1))
        for i in range(0, len(entries), 2):
            a, b = entries[i], entries[i + 1]
            self.assertEqual((a["task"], a["run_index"]), (b["task"], b["run_index"]))
            self.assertNotEqual(a["arm"], b["arm"])
        # over the 12 tasks of phase 1 each arm goes first equally often in every run
        for run in range(3):
            firsts = [e["arm"] for e in entries if e["run_index"] == run and e["first_arm"]]
            self.assertEqual(firsts.count(ARMS[0]), firsts.count(ARMS[1]))

    def test_only_task_keeps_global_position(self) -> None:
        pair = core.build_pair(
            TASKS, "MB-05", 0, ARMS, 1, 1
        )  # position 4, run 0 -> even -> arm A first
        self.assertEqual(pair[0]["arm"], ARMS[0])
        pair = core.build_pair(TASKS, "MB-05", 1, ARMS, 1, 1)
        self.assertEqual(pair[0]["arm"], ARMS[1])

    def test_seed_shared_by_arms_and_stable(self) -> None:
        self.assertEqual(core.probe_seed("MB-01", 0), 876957)  # the value printed by the dry run
        self.assertNotEqual(core.probe_seed("MB-01", 0), core.probe_seed("MB-01", 1))
        self.assertLess(core.probe_seed("MB-12", 4), 1_000_000)

    def test_block_status_and_incomplete_flags(self) -> None:
        blocks = core.build_blocks(TASKS[:2], ARMS, 3, 2, 1)
        rows = []
        for e in core.flat_entries(blocks):
            if e["trial_id"] == "MB-02-r3-cc-codex-cu":
                continue  # the stop rule hit mid-block
            rows.append(
                {"trial_id": e["trial_id"], "final": True, "smoke": False, "excluded": False}
            )
            if e["trial_id"] == "MB-01-r1-cc-cua-driver":
                rows.append(
                    {"trial_id": e["trial_id"], "final": False, "smoke": False, "excluded": True}
                )  # earlier attempt
        status = core.block_status(rows, blocks)
        self.assertTrue(status["P1-MB-01"]["complete"])
        self.assertFalse(status["P1-MB-02"]["complete"])
        self.assertEqual(status["P1-MB-02"]["n_trials"], 5)
        flagged = core.flag_incomplete(rows, blocks)
        self.assertTrue(all(not r["block_incomplete"] for r in flagged if r["block"] == "P1-MB-01"))
        self.assertTrue(all(r["block_incomplete"] for r in flagged if r["block"] == "P1-MB-02"))
        self.assertTrue(
            all(not r["block_incomplete"] for r in flagged if r["block"] == "P2-MB-02")
        )  # a later complete block stands alone

    def test_smoke_rows_never_count(self) -> None:
        blocks = core.build_blocks(TASKS[:1], ARMS, 1, 0, 1)
        rows = [
            {"trial_id": e["trial_id"], "final": True, "smoke": True}
            for e in core.flat_entries(blocks)
        ]
        self.assertFalse(core.block_status(rows, blocks)["P1-MB-01"]["complete"])
        self.assertEqual(core.completed_trial_ids(rows), set())

    def test_phase2_gate(self) -> None:
        blocks = core.build_blocks(TASKS[:2], ARMS, 1, 1, 1)
        p1 = {e["trial_id"] for b in blocks if b["phase"] == 1 for e in b["entries"]}
        self.assertTrue(core.phase2_allowed(blocks, p1, True))
        self.assertFalse(core.phase2_allowed(blocks, p1, False))
        self.assertFalse(core.phase2_allowed(blocks, set(list(p1)[:-1]), True))


class QuotaTest(unittest.TestCase):
    def test_go_without_data_and_below_threshold(self) -> None:
        self.assertEqual(core.quota_gate(None)["action"], "go")
        self.assertEqual(
            core.quota_gate({"seven_day": 0.93, "five_hour": 0.7, "status": "allowed_warning"})[
                "action"
            ],
            "go",
        )

    def test_stop_at_ninety_five_percent_seven_day(self) -> None:
        self.assertEqual(
            core.quota_gate({"seven_day": 0.95, "status": "allowed_warning"})["action"], "stop"
        )
        self.assertEqual(core.quota_gate({"seven_day": 0.97})["action"], "stop")

    def test_rejected_seven_day_stops_rejected_five_hour_waits(self) -> None:
        self.assertEqual(
            core.quota_gate({"status": "rejected", "type": "seven_day", "seven_day": 0.9})[
                "action"
            ],
            "stop",
        )
        gate = core.quota_gate(
            {
                "status": "rejected",
                "type": "five_hour",
                "seven_day": 0.9,
                "five_hour_resets_at": 1791257400.0,
            }
        )
        self.assertEqual(gate["action"], "wait")
        self.assertEqual(gate["wait_until"], 1791257400.0)
        # a rejected five-hour window with the seven-day window also over the line is a stop
        self.assertEqual(
            core.quota_gate({"status": "rejected", "type": "five_hour", "seven_day": 0.96})[
                "action"
            ],
            "stop",
        )


class TimingTest(unittest.TestCase):
    def test_backoff_doubles_and_caps(self) -> None:
        self.assertEqual([core.backoff_seconds(n) for n in range(6)], [60, 120, 240, 480, 900, 900])

    def test_wait_until_reset_with_cap_and_margin(self) -> None:
        self.assertEqual(core.wait_seconds(0, 1000.0 + 600, now=1000.0), 630.0)
        self.assertEqual(
            core.wait_seconds(0, 1000.0 + 10 * 3600, now=1000.0), core.RESET_WAIT_MAX_S
        )
        self.assertEqual(core.wait_seconds(2, None, now=1000.0), 240.0)
        self.assertEqual(
            core.wait_seconds(1, 900.0, now=1000.0), 120.0
        )  # reset already past -> backoff

    def test_cutoff(self) -> None:
        cut = core.parse_utc("2026-10-06T03:00:00Z")
        self.assertFalse(core.past_cutoff(cut, cut - 1))
        self.assertTrue(core.past_cutoff(cut, cut))
        self.assertFalse(core.past_cutoff(None, 1e12))
        self.assertEqual(core.parse_utc("2026-10-06T03:00:00"), cut)
        self.assertEqual(core.parse_utc("2026-10-06T05:00:00+02:00"), cut)

    def test_block_fits(self) -> None:
        cut = 10_000.0
        self.assertTrue(core.block_fits(cut, 100.0, now=9_000.0))
        self.assertFalse(core.block_fits(cut, 1000.0, now=9_000.0))
        self.assertFalse(core.block_fits(cut, None, now=0.0))
        self.assertTrue(core.block_fits(None, None))


class LedgerTest(unittest.TestCase):
    def test_append_cumulative_and_budget_gate(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ledger = core.Ledger(Path(tmp) / "ledger" / "spend.jsonl")
            ledger.append("W2", "smoke", "claude-haiku-4-5", 0.25)
            ledger.append("runner", "trial", "claude-sonnet-5-5", 1.5, run_id="x")
            ledger.append("runner", "trial", "claude-sonnet-5-5", None)
            self.assertAlmostEqual(ledger.cumulative(), 1.75)
            self.assertAlmostEqual(ledger.cumulative("W2"), 0.25)
            entry = ledger.entries()[0]
            self.assertEqual(set(entry) >= {"ts", "who", "purpose", "model", "cost_usd"}, True)
            self.assertTrue(core.budget_allows(1e9, 6.0, None))  # no cap by default
            self.assertTrue(core.budget_allows(10.0, 4.0, 15.0))
            self.assertFalse(core.budget_allows(12.0, 4.0, 15.0))


class PinsAndGateTest(unittest.TestCase):
    def test_compare_pins_flags_every_difference_and_missing_values(self) -> None:
        expected = {"a": "1", "b": "2", "c": "3", "release": "url", "cua_driver_git_sha": "x"}
        rows = core.compare_pins(expected, {"a": "1", "b": "9"})
        self.assertEqual(
            {name: ok for name, _, _, ok in rows}, {"a": True, "b": False, "c": False}
        )  # informational keys skipped

    def test_pins_json_covers_the_pins_the_lead_listed(self) -> None:
        pins = ca.load_pins()
        for key in (
            "cua_driver_binary_sha256",
            "cua_driver_tarball_sha256",
            "cua_skills_tarball_sha256",
            "cua_skills_tree_sha256",
            "claude_code_version",
            "macos_version",
            "macos_build",
            "chatgpt_app_version",
            "unified_computer_use_plugin_version",
            "codex_computer_use_service_version",
            "cua_driver_version_string",
        ):
            self.assertIn(key, pins)
        self.assertEqual(pins["cua_driver_version"], "0.34.0")
        self.assertEqual(pins["chatgpt_app_version"], "26.930.51102")

    def test_gate_decision(self) -> None:
        now = 1_000_000.0
        self.assertEqual(core.gate_decision(None, now=now)["action"], "unknown")
        self.assertEqual(
            core.gate_decision({"seven_day": 0.94, "ts": "t"}, now=now)["action"], "go"
        )
        gated = core.gate_decision(
            {"seven_day": 0.95, "ts": "t", "seven_day_resets_at": now + 100}, now=now
        )
        self.assertEqual(gated["action"], "gated")
        self.assertIn(">= 0.95", gated["reason"])
        # the reading belongs to a window that has since reset: no longer a gate
        self.assertEqual(
            core.gate_decision(
                {"seven_day": 0.97, "ts": "t", "seven_day_resets_at": now - 5}, now=now
            )["action"],
            "unknown",
        )
        # without a reset time a high reading still gates
        self.assertEqual(
            core.gate_decision({"seven_day": 0.96, "ts": "t"}, now=now)["action"], "gated"
        )

    def test_latest_known_quota_uses_the_newest_source(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            latest = Path(tmp) / "quota_latest.json"
            ledger = Path(tmp) / "spend.jsonl"
            self.assertIsNone(core.latest_known_quota(latest, ledger))
            ledger.write_text(
                json.dumps(
                    {"ts": "2026-10-05T23:00:00+00:00", "purpose": "p", "seven_day_after": 0.91}
                )
                + "\n"
                + json.dumps({"ts": "2026-10-05T23:10:00+00:00", "purpose": "q"})
                + "\n"
            )
            self.assertEqual(core.latest_known_quota(latest, ledger)["seven_day"], 0.91)
            core.persist_quota(
                latest, {"seven_day": 0.96, "five_hour": 0.5, "status": "allowed_warning"}, "test"
            )
            got = core.latest_known_quota(latest, ledger)
            self.assertEqual(got["seven_day"], 0.96)  # the file is newer than the ledger entry
            self.assertEqual(got["source"], "test")


def _stream(events: list[dict], stamp0: float = 1000.0) -> list[dict]:
    out = []
    for i, event in enumerate(events):
        event = dict(event)
        event["_ms"] = stamp0 + i * 100
        out.append(event)
    return out


INIT = {
    "type": "system",
    "subtype": "init",
    "model": "claude-sonnet-5-5",
    "permissionMode": "dontAsk",
    "tools": ["Read", "Skill", "ToolSearch", "mcp__cua__click", "mcp__cua__get_window_state"],
    "mcp_servers": [{"name": "cua", "status": "connected"}],
    "skills": ["cua-driver"],
    "plugins": [],
    "claude_code_version": "2.1.289",
}


def _assistant(mid: str, content: list[dict], usage: dict | None = None) -> dict:
    return {
        "type": "assistant",
        "message": {
            "id": mid,
            "content": content,
            "usage": usage
            or {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_read_input_tokens": 1000,
                "cache_creation_input_tokens": 200,
            },
        },
    }


class StreamParsingTest(unittest.TestCase):
    def _events(self) -> list[dict]:
        return _stream(
            [
                INIT,
                _assistant(
                    "m1",
                    [
                        {
                            "type": "tool_use",
                            "id": "t1",
                            "name": "ToolSearch",
                            "input": {"query": "x"},
                        }
                    ],
                ),
                {
                    "type": "user",
                    "message": {
                        "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
                    },
                },
                _assistant(
                    "m2",
                    [
                        {
                            "type": "tool_use",
                            "id": "t2",
                            "name": "mcp__cua__get_window_state",
                            "input": {},
                        }
                    ],
                ),
                {
                    "type": "user",
                    "message": {
                        "content": [{"type": "tool_result", "tool_use_id": "t2", "content": "tree"}]
                    },
                },
                _assistant(
                    "m3",
                    [
                        {
                            "type": "tool_use",
                            "id": "t3",
                            "name": "mcp__cua__click",
                            "input": {"x": 1},
                        }
                    ],
                ),
                {
                    "type": "user",
                    "message": {
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "t3",
                                "is_error": True,
                                "content": [{"type": "text", "text": "no such window"}],
                            }
                        ]
                    },
                },
                {
                    "type": "rate_limit_event",
                    "rate_limit_info": {
                        "status": "allowed_warning",
                        "rateLimitType": "seven_day",
                        "resetsAt": 1791406800,
                        "unifiedWindows": {
                            "five_hour": {"utilization": 0.4, "resetsAt": 1791257400},
                            "seven_day": {"utilization": 0.91, "resetsAt": 1791406800},
                        },
                    },
                },
                _assistant("m4", [{"type": "text", "text": "DONE"}]),
                {
                    "type": "result",
                    "subtype": "success",
                    "is_error": False,
                    "num_turns": 4,
                    "duration_ms": 5000,
                    "duration_api_ms": 4000,
                    "total_cost_usd": 0.42,
                    "result": "DONE",
                    "usage": {
                        "input_tokens": 40,
                        "output_tokens": 20,
                        "cache_read_input_tokens": 4000,
                        "cache_creation_input_tokens": 800,
                    },
                    "terminal_reason": "completed",
                    "permission_denials": [],
                },
            ]
        )

    def test_counts_tokens_cost_quota_and_latency(self) -> None:
        summary = ce.summarize(self._events())
        calls = summary["tool_calls"]
        self.assertEqual(calls["total"], 3)
        self.assertEqual(calls["mcp"], 2)
        self.assertEqual(calls["builtin"], 1)
        self.assertEqual(calls["by_name"]["mcp__cua__click"], 1)
        self.assertEqual(calls["by_class"]["observe"], 1)
        self.assertEqual(calls["by_class"]["click"], 1)
        self.assertEqual(calls["failed"], 1)
        self.assertEqual(calls["failed_by_name"], {"mcp__cua__click": 1})
        self.assertEqual(summary["turns"], 4)
        self.assertEqual(
            summary["tokens"], {"input": 40, "output": 20, "cache_read": 4000, "cache_write": 800}
        )
        self.assertEqual(summary["token_source"], "result")
        self.assertEqual(summary["tokens_summed_messages"]["input"], 40)
        self.assertEqual(summary["total_cost_usd"], 0.42)
        self.assertEqual(summary["final_text"], "DONE")
        self.assertEqual(summary["baseline_prompt_tokens"], 1210)
        self.assertAlmostEqual(summary["quota_last"]["seven_day"], 0.91)
        self.assertAlmostEqual(summary["quota_last"]["five_hour"], 0.4)
        self.assertEqual(summary["quota_last"]["status"], "allowed_warning")
        self.assertEqual(summary["action_latency_ms"]["observe"]["median"], 100.0)
        self.assertEqual(summary["init"]["tools_builtin"], ["Read", "Skill", "ToolSearch"])
        self.assertTrue(summary["init"]["tool_search_available"])
        self.assertEqual(summary["init"]["mcp_tool_count"], 2)

    def test_killed_trial_without_result_uses_per_message_usage(self) -> None:
        events = self._events()[:-1]
        summary = ce.summarize(events)
        self.assertFalse(summary["has_result"])
        self.assertEqual(summary["token_source"], "per_message_sum")
        self.assertEqual(summary["tokens"]["output"], 20)
        self.assertIsNone(summary["total_cost_usd"])

    def test_streamed_chunks_of_one_message_count_once(self) -> None:
        events = _stream(
            [
                INIT,
                _assistant("m1", [{"type": "text", "text": "a"}]),
                _assistant("m1", [{"type": "text", "text": "a"}]),
            ]
        )
        self.assertEqual(ce.summarize(events)["turns"], 1)

    def test_read_events_accepts_stamped_bare_and_torn_lines(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "s.tsv"
            path.write_text(
                "1000.0\t"
                + json.dumps({"type": "system"})
                + "\n"
                + json.dumps({"type": "result"})
                + '\n1200.0\t{"type": "assis'
            )
            events = ce.read_events(path)
            self.assertEqual([e["type"] for e in events], ["system", "result"])
            self.assertEqual(events[0]["_ms"], 1000.0)
            self.assertNotIn("_ms", events[1])

    def test_codex_repl_js_calls_are_classed_from_the_source(self) -> None:
        self.assertEqual(
            ce.tool_class("mcp__codex-cu__js", {"code": "await cua.typeText('x')"}), "type"
        )
        self.assertEqual(
            ce.tool_class(
                "mcp__codex-cu__js", {"code": "const a = await cua.getApp('Calculator')"}
            ),
            "observe",
        )
        self.assertEqual(
            ce.tool_class("mcp__codex-cu__js", {"code": "await app.click({x:1,y:2})"}), "click"
        )
        self.assertEqual(ce.tool_class("Read", {}), "builtin")


class FailureClassificationTest(unittest.TestCase):
    def _summary(
        self, result: dict | None, quota: list[dict] | None = None, init: dict | None = INIT
    ) -> dict:
        events = [init] if init else []
        events += [{"type": "rate_limit_event", "rate_limit_info": q} for q in (quota or [])]
        if result is not None:
            events.append({"type": "result", **result})
        return ce.summarize(events)

    def test_rejected_rate_limit_event(self) -> None:
        summary = self._summary(
            {"is_error": True, "result": "limit"},
            [
                {
                    "status": "rejected",
                    "rateLimitType": "five_hour",
                    "resetsAt": 1791257400,
                    "unifiedWindows": {"five_hour": {"utilization": 1.0, "resetsAt": 1791257400}},
                }
            ],
        )
        out = ce.classify_failure(summary)
        self.assertEqual(out["kind"], "rate_limit")
        self.assertEqual(out["reset_epoch"], 1791257400.0)

    def test_limit_text_with_reset_epoch(self) -> None:
        out = ce.classify_failure(
            self._summary({"is_error": True, "result": "Claude AI usage limit reached|1791257400"})
        )
        self.assertEqual(out["kind"], "rate_limit")
        self.assertEqual(out["reset_epoch"], 1791257400.0)

    def test_429_and_overloaded_and_auth(self) -> None:
        self.assertEqual(
            ce.classify_failure(
                self._summary({"is_error": True, "api_error_status": 429, "result": "x"})
            )["kind"],
            "rate_limit",
        )
        self.assertEqual(
            ce.classify_failure(
                self._summary({"is_error": True, "api_error_status": 529, "result": "Overloaded"})
            )["kind"],
            "overloaded",
        )
        self.assertEqual(
            ce.classify_failure(
                self._summary(
                    {"is_error": True, "api_error_status": 401, "result": "Invalid API key"}
                )
            )["kind"],
            "auth",
        )

    def test_clean_success_and_agent_failures_are_not_infra(self) -> None:
        self.assertIsNone(
            ce.classify_failure(self._summary({"is_error": False, "result": "DONE"}))["kind"]
        )
        # running out of turns is the agent's failure, not infrastructure
        self.assertIsNone(
            ce.classify_failure(
                self._summary({"is_error": True, "subtype": "error_max_turns", "result": ""})
            )["kind"]
        )

    def test_mcp_server_not_connected_and_crash_before_init(self) -> None:
        bad = dict(INIT, mcp_servers=[{"name": "cua", "status": "failed"}])
        self.assertEqual(
            ce.classify_failure(self._summary({"is_error": False, "result": "BLOCKED"}, init=bad))[
                "kind"
            ],
            "mcp_start",
        )
        self.assertEqual(
            ce.classify_failure(self._summary(None, init=None), "boom", 1)["kind"], "harness_crash"
        )


class ArmsAndHostTest(unittest.TestCase):
    def test_env_is_scrubbed_to_the_allowlist(self) -> None:
        saved = dict(os.environ)
        try:
            os.environ.update(
                {
                    "CLAUDE_CODE_ENTRYPOINT": "cli",
                    "CLAUDECODE": "1",
                    "ANTHROPIC_API_KEY": "must-not-leak",
                    "LITELLM_MASTER_KEY": "x",
                }
            )
            env = ca.claude_env()
        finally:
            os.environ.clear()
            os.environ.update(saved)
        self.assertEqual(
            set(env)
            - {
                "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
                "CLAUDE_CODE_DISABLE_CLAUDE_MDS",
                "DISABLE_AUTOUPDATER",
            }
            <= set(ca.CLAUDE_ENV_ALLOW),
            True,
        )
        self.assertNotIn("ANTHROPIC_API_KEY", env)
        self.assertNotIn("CLAUDECODE", env)
        self.assertNotIn("CLAUDE_CODE_ENTRYPOINT", env)
        self.assertEqual(env["CLAUDE_CODE_DISABLE_AUTO_MEMORY"], "1")
        self.assertEqual(env["CLAUDE_CODE_DISABLE_CLAUDE_MDS"], "1")

    def test_argv_is_identical_across_arms_except_mcp(self) -> None:
        common = dict(model="claude-sonnet-5-5", max_turns=30, max_budget_usd=6.0)
        a = ca.claude_argv(mcp_config=Path("/x/a.json"), server="cua", **common)
        b = ca.claude_argv(mcp_config=Path("/x/b.json"), server="codex-cu", **common)
        diff = [(x, y) for x, y in zip(a, b) if x != y]
        self.assertEqual(diff, [("/x/a.json", "/x/b.json"), ("mcp__cua", "mcp__codex-cu")])
        self.assertEqual(len(a), len(b))
        joined = " ".join(a)
        for flag in (
            "--strict-mcp-config",
            "--no-session-persistence",
            "--setting-sources project",
            "--permission-mode dontAsk",
            "--tools Skill,Read,ToolSearch",
            "--input-format stream-json",
            "--output-format stream-json",
            "--verbose",
            "--max-turns 30",
            "--max-budget-usd 6",
            "--model claude-sonnet-5-5",
        ):
            self.assertIn(flag, joined)
        self.assertNotIn("Bash", joined)
        self.assertNotIn("dangerously", joined)
        off = ca.claude_argv(
            mcp_config=Path("/x/a.json"), server="cua", tool_search=False, **common
        )
        self.assertIn("--tools Skill,Read", " ".join(off))
        self.assertNotIn("ToolSearch", " ".join(off))

    def test_elicitation_allowlist(self) -> None:
        allowed = {"benchlab", "calculator"}
        self.assertEqual(
            claude_driver.answer_elicitation('Allow Computer Use to use "BenchLab"?', allowed)[
                "action"
            ],
            "accept",
        )
        self.assertEqual(
            claude_driver.answer_elicitation('Allow Computer Use to use "Calculator"?', allowed)[
                "action"
            ],
            "accept",
        )
        self.assertEqual(
            claude_driver.answer_elicitation('Allow Computer Use to use "Safari"?', allowed)[
                "action"
            ],
            "decline",
        )
        self.assertEqual(
            claude_driver.answer_elicitation("Something else", allowed)["action"], "decline"
        )

    def test_arm_a_mcp_config_uses_the_absolute_private_binary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path, server = ca.mcp_config_for("cc-cua-driver", Path(tmp))
            data = json.loads(path.read_text())
            entry = data["mcpServers"][server]
            self.assertTrue(os.path.isabs(entry["command"]))
            self.assertEqual(entry["command"], str(ca.CUA_BIN))
            self.assertIn("0.34.0", entry["command"])
            self.assertEqual(entry["args"], ["--socket", ca.AGENT_SOCKET, "mcp"])
            self.assertEqual(entry["env"]["CUA_DRIVER_RS_TELEMETRY_ENABLED"], "false")

    def test_main_build_arm_has_its_own_binary_socket_and_state(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path, server = ca.mcp_config_for("cc-cua-driver-main", Path(tmp))
            entry = json.loads(path.read_text())["mcpServers"][server]
            self.assertEqual(server, "cua")
            self.assertEqual(entry["command"], str(ca.CUA_MAIN_APP / "Contents/MacOS/cua-driver"))
            self.assertEqual(entry["args"], ["--socket", ca.MAIN_SOCKET, "mcp"])
            self.assertNotEqual(ca.MAIN_SOCKET, ca.AGENT_SOCKET)
            self.assertEqual(entry["env"]["DO_NOT_TRACK"], "1")

    def test_three_arms_rotate_the_first_arm(self) -> None:
        arms3 = ["cc-cua-driver-main", "cc-cua-driver", "cc-codex-cu"]
        firsts = {core.arm_order(0, r, arms3)[0] for r in range(3)}
        self.assertEqual(firsts, set(arms3))

    def test_cwd_gets_the_skill_only_for_arm_a(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp) / "work"
            ca.prepare_cwd("cc-cua-driver", cwd)
            self.assertTrue((cwd / ".claude/skills/cua-driver/SKILL.md").is_file())
            ca.prepare_cwd("cc-codex-cu", cwd)
            self.assertFalse((cwd / ".claude").exists())


if __name__ == "__main__":
    unittest.main()
