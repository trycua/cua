"""Flow tests for run_bench.execute_trial and the gates, with run_attempt and the pauses faked: retries,
backoff, exclusion, the quota stop rule, the wall-clock cutoff, the optional budget cap, resume and
finalize. No GUI, no model."""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import bench_core as core  # noqa: E402
import run_bench as rb  # noqa: E402

ENTRY = {
    "trial_id": "MB-01-r1-cc-cua-driver",
    "phase": 1,
    "block": "P1-MB-01",
    "run_index": 0,
    "run": 1,
    "task": "MB-01",
    "arm": "cc-cua-driver",
    "task_pos": 0,
    "arm_slot": 0,
    "first_arm": True,
    "schedule_seed": 1,
    "order_index": 0,
}


def make_ctx(tmp: str, **overrides: object) -> rb.Ctx:
    args = argparse.Namespace(
        ledger_who="test",
        model="m",
        max_budget_usd=6.0,
        budget_cap_usd=None,
        seven_day_stop=0.95,
        smoke=False,
        phase1_runs=1,
        phase2_runs=0,
        schedule_seed=1,
        build_dir=None,
    )
    for key, value in overrides.items():
        setattr(args, key, value)
    run_dir = Path(tmp) / "run"
    run_dir.mkdir()
    ctx = rb.Ctx(
        args,
        run_dir,
        core.Ledger(Path(tmp) / "spend.jsonl"),
        {},
        ["MB-01"],
        ["cc-cua-driver", "cc-codex-cu"],
        full_task_ids=["MB-01"],
    )
    return ctx


def attempt_row(
    attempt: int,
    infra: str | None = None,
    cost: float = 0.5,
    quota7: float | None = 0.9,
    **extra: object,
) -> dict:
    row = {
        "trial_id": ENTRY["trial_id"],
        "attempt": attempt,
        "final": False,
        "smoke": False,
        "arm": ENTRY["arm"],
        "status": "infra_error" if infra else "completed",
        "passed": not infra,
        "wall_s": 1.0,
        "turns": 3,
        "cost_usd": cost,
        "infra_failure": infra,
        "infra_message": infra or "",
        "excluded": bool(infra),
        "quota_seven_day_after": quota7,
    }
    row.update(extra)
    return row


class ExecuteTrialTest(unittest.TestCase):
    def run_trial(
        self, ctx: rb.Ctx, rows: list[dict]
    ) -> tuple[dict | None, list[tuple[str, float]], Exception | None]:
        pauses: list[tuple[str, float]] = []
        calls = iter(rows)

        def fake_attempt(_ctx: rb.Ctx, _entry: dict, attempt: int) -> dict:
            row = next(calls)
            row["attempt"] = attempt
            return row

        def fake_pause(
            _ctx: rb.Ctx, kind: str, seconds: float, message: str, trial_id: str | None
        ) -> None:
            pauses.append((kind, seconds))

        with (
            mock.patch.object(rb, "run_attempt", fake_attempt),
            mock.patch.object(rb, "log_pause", fake_pause),
        ):
            try:
                return rb.execute_trial(ctx, dict(ENTRY)), pauses, None
            except rb.StopRun as stop:
                return None, pauses, stop

    def test_clean_trial_is_final_and_written_to_results_and_ledger(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            row, pauses, err = self.run_trial(ctx, [attempt_row(1)])
            self.assertIsNone(err)
            self.assertTrue(row["final"])
            self.assertEqual(pauses, [])
            saved = core.load_rows(ctx.results_path)
            self.assertEqual(len(saved), 1)
            self.assertAlmostEqual(ctx.ledger.cumulative(), 0.5)

    def test_rate_limit_waits_then_retries_the_same_trial_id_without_counting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            rows = [attempt_row(1, "rate_limit"), attempt_row(2, "rate_limit"), attempt_row(3)]
            row, pauses, err = self.run_trial(ctx, rows)
            self.assertIsNone(err)
            self.assertEqual([k for k, _ in pauses], ["retry_rate_limit", "retry_rate_limit"])
            self.assertEqual([s for _, s in pauses], [60.0, 120.0])  # exponential backoff from 60 s
            saved = core.load_rows(ctx.results_path)
            self.assertEqual([r["final"] for r in saved], [False, False, True])
            self.assertEqual({r["trial_id"] for r in saved}, {ENTRY["trial_id"]})
            self.assertEqual(core.completed_trial_ids(saved), {ENTRY["trial_id"]})

    def test_rate_limit_with_reset_time_waits_until_reset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            reset = time.time() + 1800
            _, pauses, _ = self.run_trial(
                ctx, [attempt_row(1, "rate_limit", infra_reset_epoch=reset), attempt_row(2)]
            )
            self.assertAlmostEqual(pauses[0][1], 1830, delta=5)

    def test_seven_day_rejected_stops_instead_of_waiting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.quota = {"status": "rejected", "type": "seven_day", "seven_day": 0.97}
            row, pauses, err = self.run_trial(ctx, [attempt_row(1, "rate_limit")])
            self.assertIsInstance(err, rb.StopRun)
            self.assertEqual(err.reason, "QUOTA")
            self.assertEqual(pauses, [])

    def test_overloaded_retries_three_times_then_excludes_and_moves_on(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            rows = [attempt_row(i, "overloaded") for i in range(1, 5)]
            row, pauses, err = self.run_trial(ctx, rows)
            self.assertIsNone(err)
            self.assertTrue(row["final"])
            self.assertTrue(row["excluded"])
            self.assertEqual([s for _, s in pauses], [60.0, 120.0, 240.0])
            self.assertEqual(len(core.load_rows(ctx.results_path)), 4)

    def test_other_infra_failure_is_retried_once(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            row, pauses, err = self.run_trial(
                ctx, [attempt_row(1, "mcp_start"), attempt_row(2, "mcp_start")]
            )
            self.assertIsNone(err)
            self.assertTrue(row["final"] and row["excluded"])
            self.assertEqual(len(pauses), 1)

    def test_auth_failure_stops_the_run_after_one_retry(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            _, pauses, err = self.run_trial(ctx, [attempt_row(1, "auth"), attempt_row(2, "auth")])
            self.assertIsInstance(err, rb.StopRun)
            self.assertEqual(err.reason, "AUTH")
            self.assertEqual(len(pauses), 1)

    def test_agent_failures_are_not_retried(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            failed = attempt_row(1, None, passed=False, status="timeout")
            row, pauses, err = self.run_trial(ctx, [failed])
            self.assertTrue(row["final"])
            self.assertEqual(pauses, [])
            self.assertFalse(row["passed"])


class GateTest(unittest.TestCase):
    def test_quota_stop_rule_blocks_new_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.quota = {
                "seven_day": 0.95,
                "five_hour": 0.2,
                "status": "allowed_warning",
                "type": "seven_day",
            }
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "QUOTA")
            ctx.quota = {"seven_day": 0.949, "status": "allowed_warning", "type": "seven_day"}
            rb.check_gates(ctx)  # below the line: no exception

    def test_cutoff_blocks_new_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.cutoff = time.time() - 1
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "TIME")

    def test_stop_file_blocks_new_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            (ctx.run_dir / "STOP").write_text("x")
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "USER")

    def test_optional_budget_cap_only_when_set(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.ledger.append("x", "spent", "m", 148.0)
            rb.check_gates(ctx)  # no cap by default
            ctx.args.budget_cap_usd = 150.0
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "BUDGET")

    def test_five_hour_rejected_waits_for_the_reset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.quota = {
                "status": "rejected",
                "type": "five_hour",
                "seven_day": 0.9,
                "five_hour_resets_at": time.time() + 600,
            }
            pauses: list[float] = []
            with mock.patch.object(
                rb, "log_pause", lambda c, kind, seconds, message, trial: pauses.append(seconds)
            ):
                rb.check_gates(ctx)
            self.assertEqual(len(pauses), 1)
            self.assertAlmostEqual(pauses[0], 630, delta=5)

    def test_five_hour_wait_that_passes_the_cutoff_stops(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp)
            ctx.cutoff = time.time() + 300
            ctx.quota = {
                "status": "rejected",
                "type": "five_hour",
                "seven_day": 0.9,
                "five_hour_resets_at": time.time() + 3600,
            }
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "TIME")


class FinalizeTest(unittest.TestCase):
    def test_finalize_flags_incomplete_blocks_and_keeps_rows(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp, phase1_runs=1, phase2_runs=0)
            blocks = ctx.blocks()
            entries = core.flat_entries(blocks)
            self.assertEqual(len(entries), 2)
            rows = [
                {
                    "trial_id": entries[0]["trial_id"],
                    "final": True,
                    "smoke": False,
                    "excluded": False,
                    "arm": entries[0]["arm"],
                    "task": "MB-01",
                }
            ]
            ctx.results_path.write_text("".join(json.dumps(r) + "\n" for r in rows))
            summary = rb.finalize(ctx)
            self.assertEqual(summary["blocks_complete"], 0)
            saved = core.load_rows(ctx.results_path)
            self.assertEqual(len(saved), 1)
            self.assertTrue(saved[0]["block_incomplete"])
            rows.append(
                {
                    "trial_id": entries[1]["trial_id"],
                    "final": True,
                    "smoke": False,
                    "excluded": False,
                    "arm": entries[1]["arm"],
                    "task": "MB-01",
                }
            )
            ctx.results_path.write_text("".join(json.dumps(r) + "\n" for r in rows))
            self.assertEqual(rb.finalize(ctx)["blocks_complete"], 1)
            self.assertFalse(any(r["block_incomplete"] for r in core.load_rows(ctx.results_path)))

    def test_resume_skips_final_ids_only(self) -> None:
        rows = [
            {"trial_id": "a", "final": True},
            {"trial_id": "b", "final": False},
            {"trial_id": "c", "smoke": True, "final": True},
        ]
        self.assertEqual(core.completed_trial_ids(rows), {"a"})


class MiscTest(unittest.TestCase):
    def test_strip_bulk_removes_screenshots_but_keeps_text(self) -> None:
        big = "A" * 50000
        out = rb.strip_bulk(
            {"type": "image", "source": {"data": big}, "text": "keep", "list": [big, "x"]}
        )
        self.assertIn("stripped", out["source"]["data"])
        self.assertEqual(out["text"], "keep")
        self.assertLess(len(json.dumps(out)), 1000)

    def test_estimate_cost_uses_cache_prices(self) -> None:
        cost = rb.estimate_cost(
            {"input": 1_000_000, "output": 0, "cache_read": 1_000_000, "cache_write": 0}
        )
        self.assertAlmostEqual(cost, 3.30)

    def test_merge_quota_keeps_known_windows(self) -> None:
        merged = rb.merge_quota(
            {"five_hour": 0.5, "seven_day": 0.9},
            {"five_hour": 0.6, "seven_day": None, "status": "allowed"},
        )
        self.assertEqual(merged["five_hour"], 0.6)
        self.assertEqual(merged["seven_day"], 0.9)
        self.assertEqual(rb.merge_quota(None, None), None)


if __name__ == "__main__":
    unittest.main()
