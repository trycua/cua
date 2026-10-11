from __future__ import annotations

import contextlib
import copy
import importlib.util
import io
import json
import math
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "analyze.py"
SPEC = importlib.util.spec_from_file_location("macos_pilot_analyze_under_test", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
az = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = az
SPEC.loader.exec_module(az)

A = az.ARM_A
B = az.ARM_B

_counter = {"n": 0}


def make_row(arm, task, run, passed, **kw):
    """Build a valid cdb-pilot-trial/1 row; keyword args override top-level fields."""
    _counter["n"] += 1
    row = {
        "schema": "cdb-pilot-trial/1",
        "trial_id": f"{arm}-{task}-{run}-{_counter['n']}",
        "arm": arm,
        "task": task,
        "task_group": "bench",
        "dimension_tags": [],
        "run_index": run,
        "seed": 1,
        "order_index": _counter["n"],
        "status": "completed",
        "excluded": False,
        "excluded_reason": None,
        "passed": bool(passed),
        "score": 1.0 if passed else 0.0,
        "wall_s": 10.0 + run,
        "agent_wall_s": 8.0 + run,
        "tool_calls": {
            "total": 10,
            "by_class": {
                "observe": 5,
                "click": 3,
                "type": 1,
                "key": 1,
                "scroll": 0,
                "drag": 0,
                "set_value": 0,
                "other": 0,
            },
            "failed": 0,
        },
        "steps": 10,
        "action_latency_ms": {"click": {"n": 3, "median": 100.0, "p90": 150.0}},
        "tokens": {"input": 1000, "cached_input": 400, "output": 200, "reasoning": 50},
        "est_cost_usd": 0.05,
        "disturbance": {
            "available": True,
            "front_changes": 0,
            "key_loss": 0,
            "keystrokes_leaked": 0,
            "clicks_leaked": 0,
            "scrolls_leaked": 0,
            "pointer_max_deviation_px": 0.0,
            "pointer_deviation_episodes": 0,
            "hid_events": {"move": 0, "down": 0, "key": 0, "scroll": 0},
            "human_input_suspected": False,
        },
        "confirmation_requested": False,
        "evaluator_read_suspected": False,
        "notes": "",
    }
    row.update(kw)
    return row


def block(arm, task, outcomes, **kw):
    """One row per outcome (1/0), run_index 1..n."""
    return [make_row(arm, task, i + 1, bool(o), **kw) for i, o in enumerate(outcomes)]


def run(rows, **kw):
    kw.setdefault("bootstrap", 400)
    return az.analyze(rows, **kw)


class WilsonTests(unittest.TestCase):
    def test_zero_of_ten(self):
        lo, hi = az.wilson_interval(0, 10)
        self.assertEqual(lo, 0.0)
        self.assertAlmostEqual(hi, 0.27753, places=4)

    def test_ten_of_ten(self):
        lo, hi = az.wilson_interval(10, 10)
        self.assertAlmostEqual(lo, 0.72247, places=4)
        self.assertEqual(hi, 1.0)

    def test_five_of_ten(self):
        lo, hi = az.wilson_interval(5, 10)
        self.assertAlmostEqual(lo, 0.23659, places=4)
        self.assertAlmostEqual(hi, 0.76341, places=4)

    def test_symmetry_and_empty(self):
        lo, hi = az.wilson_interval(3, 20)
        lo2, hi2 = az.wilson_interval(17, 20)
        self.assertAlmostEqual(lo, 1 - hi2, places=12)
        self.assertAlmostEqual(hi, 1 - lo2, places=12)
        self.assertIsNone(az.wilson_interval(0, 0))


class FisherTests(unittest.TestCase):
    def test_lady_tasting_tea(self):
        # [[3, 1], [1, 3]]: two-sided p = 34/70
        self.assertAlmostEqual(az.fisher_exact_two_sided(3, 1, 1, 3), 34 / 70, places=12)

    def test_wikipedia_dieting_example(self):
        # [[1, 9], [11, 3]]: two-sided p ~= 0.002759
        self.assertAlmostEqual(az.fisher_exact_two_sided(1, 9, 11, 3), 0.002759, places=5)

    def test_extreme_table(self):
        self.assertAlmostEqual(
            az.fisher_exact_two_sided(10, 0, 0, 10), 2 / math.comb(20, 10), places=15
        )

    def test_identical_rows_give_one(self):
        self.assertAlmostEqual(az.fisher_exact_two_sided(2, 3, 2, 3), 1.0, places=12)

    def test_symmetric_under_swapping_rows(self):
        self.assertAlmostEqual(
            az.fisher_exact_two_sided(5, 1, 2, 4), az.fisher_exact_two_sided(2, 4, 5, 1), places=12
        )

    def test_rejects_bad_input(self):
        with self.assertRaises(ValueError):
            az.fisher_exact_two_sided(-1, 1, 1, 1)
        with self.assertRaises(ValueError):
            az.fisher_exact_two_sided(0, 0, 0, 0)


class PassKTests(unittest.TestCase):
    def test_known_values(self):
        self.assertAlmostEqual(az.pass_hat_k(3, 5, 2), 3 / 10)
        self.assertEqual(az.pass_hat_k(3, 3, 3), 1.0)
        self.assertEqual(az.pass_hat_k(2, 3, 3), 0.0)
        self.assertAlmostEqual(az.pass_hat_k(3, 5, 1), 0.6)

    def test_not_estimable_when_n_below_k(self):
        self.assertIsNone(az.pass_hat_k(2, 2, 3))

    def test_pass_k_averaged_over_tasks_in_analysis(self):
        rows = (
            block(A, "T1", [1, 1, 1, 1, 0])  # C(4,3)/C(5,3) = 4/10
            + block(A, "T2", [1, 1, 1])  # 1.0
            + block(A, "T3", [1, 1])  # n < k: omitted
            + block(B, "T1", [1, 0, 0, 0, 0])
        )
        res = run(rows, pass_k=3)
        blk = res["scopes"]["headline"]["arms"][A]
        self.assertAlmostEqual(blk["pass_k"]["value"], (0.4 + 1.0) / 2)
        self.assertEqual(blk["pass_k"]["n_tasks_used"], 2)
        self.assertEqual(blk["pass_k"]["tasks_omitted_n_lt_k"], ["T3"])
        # pass@1 is the task-macro success
        self.assertAlmostEqual(blk["pass_at_1"], (0.8 + 1.0 + 1.0) / 3)
        self.assertAlmostEqual(blk["pass_at_1"], blk["task_macro_success"])


class MacroAndExclusionTests(unittest.TestCase):
    def test_task_macro_weights_tasks_equally(self):
        rows = (
            block(A, "T1", [1])
            + block(A, "T2", [0] * 9)
            + block(B, "T1", [1])
            + block(B, "T2", [0] * 9)
        )
        res = run(rows)
        blk = res["scopes"]["headline"]["arms"][A]
        self.assertAlmostEqual(blk["task_macro_success"], 0.5)
        self.assertAlmostEqual(blk["micro_rate"], 0.1)

    def test_exclusion_drops_trial_and_counts_it(self):
        rows = (
            block(A, "T1", [1, 1])
            + [
                make_row(
                    A,
                    "T1",
                    3,
                    False,
                    status="infra_error",
                    excluded=True,
                    excluded_reason="vm crash",
                )
            ]
            + block(B, "T1", [1, 0])
        )
        res = run(rows)
        cell = res["cells"][A]["T1"]
        self.assertEqual(
            (cell["n_total"], cell["n_excluded"], cell["n"], cell["successes"]), (3, 1, 2, 2)
        )
        self.assertEqual(cell["excluded_runs"][0]["reason"], "vm crash")
        self.assertEqual(res["data_summary"]["per_arm"][A]["excluded"], 1)
        self.assertEqual(res["data_summary"]["per_arm"][A]["excluded_reasons"], {"vm crash": 1})
        self.assertEqual(res["data_summary"]["per_arm"][A]["excluded_by_task"], {"T1": 1})
        self.assertAlmostEqual(res["scopes"]["headline"]["arms"][A]["task_macro_success"], 1.0)
        md = az.render_markdown(res)
        self.assertIn("vm crash", md)

    def test_non_excluded_failures_count_and_are_never_dropped(self):
        rows = (
            block(A, "T1", [1])
            + [
                make_row(A, "T1", 2, False, status="timeout"),
                make_row(A, "T1", 3, False, status="agent_error"),
                # passed=true with a non-completed status is forced to failure
                make_row(A, "T1", 4, True, status="timeout"),
                make_row(A, "T1", 5, False, status="infra_error", excluded=False),
            ]
            + block(B, "T1", [1, 1])
        )
        res = run(rows)
        cell = res["cells"][A]["T1"]
        self.assertEqual((cell["n"], cell["successes"]), (5, 1))
        self.assertEqual(cell["failures"]["timeout"], 2)
        self.assertEqual(cell["failures"]["agent_error"], 1)
        self.assertEqual(cell["failures"]["infra_error_not_excluded"], 1)
        self.assertEqual(len(res["data_quality"]["passed_true_but_status_not_completed"]), 1)
        self.assertEqual(cell["raw"]["passed"], [1, 0, 0, 0, 0])

    def test_all_runs_excluded_for_an_arm_task_is_n_zero_not_error(self):
        rows = (
            [make_row(A, "T1", 1, False, status="infra_error", excluded=True, excluded_reason="x")]
            + block(A, "T2", [1, 0])
            + block(B, "T1", [1, 0])
            + block(B, "T2", [1, 0])
        )
        res = run(rows)
        self.assertEqual(res["cells"][A]["T1"]["n"], 0)
        self.assertIsNone(res["cells"][A]["T1"]["rate"])
        cmp_ = res["scopes"]["headline"]["comparison"]
        self.assertEqual(cmp_["paired_tasks"], ["T2"])
        self.assertEqual(cmp_["tasks_without_both_arms"], ["T1"])
        self.assertIsNone(res["paired"]["T1"]["fisher_p_two_sided"])


class HeadlineScopeTests(unittest.TestCase):
    def rows(self):
        return (
            block(A, "CDB-S01", [1, 1, 1])
            + block(B, "CDB-S01", [0, 0, 0])
            + block(A, "PROBE-X", [1, 0, 1], task_group="probe")
            + block(B, "PROBE-X", [1, 0, 1], task_group="probe")
            + block(A, "CDB-HOV", [0, 0, 0], dimension_tags=["hover", "pointer"])
            + block(B, "CDB-HOV", [1, 1, 1], dimension_tags=["hover", "pointer"])
            + block(A, "ODD", [1, 1, 1], task_group="other")
            + block(B, "ODD", [0, 0, 0], task_group="other")
        )

    def test_coverage_tag_task_is_excluded_from_headline(self):
        res = run(self.rows())
        self.assertEqual(res["scopes"]["headline"]["tasks"], ["CDB-S01", "PROBE-X"])
        self.assertEqual(res["scopes"]["coverage"]["tasks"], ["CDB-HOV"])
        self.assertEqual(res["scopes"]["other"]["tasks"], ["ODD"])
        head = res["scopes"]["headline"]
        # headline macro A: (1 + 2/3)/2; B: (0 + 2/3)/2 -> hover (B better) must not leak in
        self.assertAlmostEqual(head["comparison"]["macro_a"], (1 + 2 / 3) / 2)
        self.assertAlmostEqual(head["comparison"]["macro_b"], (0 + 2 / 3) / 2)
        self.assertAlmostEqual(head["comparison"]["diff"], 0.5)
        # but the coverage task still appears in the per-task tables
        self.assertIn("CDB-HOV", res["cells"][A])
        self.assertAlmostEqual(res["paired"]["CDB-HOV"]["diff"], -1.0)
        md = az.render_markdown(res)
        self.assertIn("Coverage-tagged tasks, excluded from the headline (1): CDB-HOV", md)
        self.assertIn("| CDB-HOV |", md)

    def test_headline_groups_and_coverage_tags_are_configurable(self):
        res = run(self.rows(), headline_groups=("bench",), coverage_tags=())
        self.assertEqual(res["scopes"]["headline"]["tasks"], ["CDB-HOV", "CDB-S01"])
        self.assertEqual(res["scopes"]["other"]["tasks"], ["ODD", "PROBE-X"])
        self.assertEqual(res["scopes"]["coverage"]["tasks"], [])

    def test_paired_task_fisher_and_diff(self):
        res = run(self.rows())
        p = res["paired"]["CDB-S01"]
        self.assertAlmostEqual(p["diff"], 1.0)
        self.assertAlmostEqual(p["fisher_p_two_sided"], 2 / 20)  # [[3,0],[0,3]] -> 2/C(6,3)
        self.assertAlmostEqual(res["paired"]["PROBE-X"]["fisher_p_two_sided"], 1.0)
        md = az.render_markdown(res)
        self.assertIn("No multiplicity", md)
        self.assertIn("tiny", md)


class VerdictAndPilotTests(unittest.TestCase):
    def test_verdict_helper(self):
        self.assertEqual(az.verdict(0.05, 0.4), "A better")
        self.assertEqual(az.verdict(-0.5, -0.01), "B better")
        self.assertEqual(az.verdict(-0.1, 0.3), "no resolvable difference")
        self.assertEqual(az.verdict(0.0, 0.3), "no resolvable difference")
        self.assertEqual(az.verdict(0.0, 0.0), "no resolvable difference")
        with self.assertRaises(ValueError):
            az.verdict(None, None)

    def test_markdown_prints_ci_n_and_pilot_sentence_for_small_cells(self):
        rows = block(A, "T1", [1, 1, 1]) + block(B, "T1", [0, 0, 1])
        res = run(rows)
        md = az.render_markdown(res)
        self.assertIn("pilot-sized: not decision-bearing", md)
        self.assertIn("95% CI", md)
        self.assertIn("n = 1 paired headline tasks (3 runs in A, 3 runs in B)", md)
        self.assertTrue(res["headline"]["pilot_sized"])
        self.assertEqual(res["headline"]["pilot_sentence"], "pilot-sized: not decision-bearing")

    def test_pilot_sentence_absent_when_every_headline_task_has_five_runs_per_arm(self):
        rows = block(A, "T1", [1, 1, 1, 1, 0]) + block(B, "T1", [0, 0, 1, 0, 0])
        res = run(rows)
        self.assertFalse(res["headline"]["pilot_sized"])
        self.assertNotIn("pilot-sized: not decision-bearing", az.render_markdown(res))

    def test_pilot_sentence_when_one_headline_task_is_small(self):
        rows = (
            block(A, "T1", [1] * 5)
            + block(B, "T1", [0] * 5)
            + block(A, "T2", [1] * 5)
            + block(B, "T2", [0] * 4)
        )
        res = run(rows)
        self.assertTrue(res["headline"]["pilot_sized"])
        self.assertEqual(
            res["scopes"]["headline"]["pilot_small_cells"], [{"task": "T2", "arm": B, "n": 4}]
        )

    def test_verdict_in_analysis_resolves_with_clear_effect(self):
        rows = []
        for t in ("T1", "T2", "T3", "T4"):
            rows += block(A, t, [1] * 8) + block(B, t, [0] * 8)
        res = run(rows, bootstrap=500)
        self.assertEqual(res["headline"]["verdict"], "A better")
        self.assertGreater(res["headline"]["ci_low"], 0)
        md = az.render_markdown(res)
        self.assertIn("Verdict: **A better**", md)

    def test_verdict_no_resolvable_difference_for_identical_arms(self):
        rows = []
        for t in ("T1", "T2", "T3"):
            rows += block(A, t, [1, 0, 1, 0, 1]) + block(B, t, [1, 0, 1, 0, 1])
        res = run(rows, bootstrap=500)
        self.assertEqual(res["headline"]["verdict"], "no resolvable difference")
        self.assertLessEqual(res["headline"]["ci_low"], 0)
        self.assertGreaterEqual(res["headline"]["ci_high"], 0)


class SingleArmTests(unittest.TestCase):
    def test_single_arm_mode(self):
        rows = block(A, "T1", [1, 0, 1]) + block(A, "T2", [1, 1, 1])
        res = run(rows)
        self.assertEqual(res["mode"], "single-arm")
        self.assertEqual(res["present_arms"], [A])
        self.assertFalse(res["headline"]["comparison_possible"])
        self.assertIsNone(res["headline"]["verdict"])
        self.assertFalse(res["scopes"]["headline"]["comparison"]["possible"])
        self.assertAlmostEqual(
            res["scopes"]["headline"]["arms"][A]["task_macro_success"], (2 / 3 + 1) / 2
        )
        self.assertIn(A, res["scopes"]["headline"]["arm_bootstrap"])
        md = az.render_markdown(res)
        self.assertIn("no comparison is possible", md)
        self.assertIn("Single-arm descriptive tables follow", md)
        self.assertIn("pilot-sized: not decision-bearing", md)
        # per-arm x task table still printed
        self.assertIn("| T1 |", md)

    def test_other_arm_only_excluded_is_single_arm(self):
        rows = block(A, "T1", [1, 0]) + [
            make_row(B, "T1", 1, False, status="infra_error", excluded=True, excluded_reason="boot")
        ]
        res = run(rows)
        self.assertEqual(res["mode"], "single-arm")
        self.assertEqual(res["data_summary"]["per_arm"][B]["excluded"], 1)

    def test_all_trials_excluded_renders_without_error(self):
        rows = [
            make_row(
                A, "T1", 1, False, status="infra_error", excluded=True, excluded_reason="boot"
            ),
            make_row(
                B, "T1", 1, False, status="infra_error", excluded=True, excluded_reason="boot"
            ),
        ]
        res = run(rows)
        self.assertEqual(res["mode"], "no-data")
        md = az.render_markdown(res)
        self.assertIn("No arm has non-excluded trials", md)
        self.assertIn("pilot-sized: not decision-bearing", md)

    def test_no_shared_headline_task_means_no_comparison(self):
        rows = block(A, "T1", [1, 1]) + block(B, "T2", [0, 0])
        res = run(rows)
        self.assertEqual(res["mode"], "two-arm")
        cmp_ = res["scopes"]["headline"]["comparison"]
        self.assertFalse(cmp_["possible"])
        self.assertIn("no comparison is possible", az.render_markdown(res))


class BootstrapTests(unittest.TestCase):
    def rows(self):
        rows = []
        for t, (pa, pb) in {
            "T1": ([1, 1, 0, 1], [1, 0, 0, 0]),
            "T2": ([1, 0, 1], [0, 0, 1, 1, 0]),
            "T3": ([1, 1], [1, 0]),
        }.items():
            rows += block(A, t, pa) + block(B, t, pb)
        return rows

    def test_same_seed_is_deterministic(self):
        rows = self.rows()
        r1 = az.analyze(copy.deepcopy(rows), bootstrap=300, seed=7)
        r2 = az.analyze(copy.deepcopy(rows), bootstrap=300, seed=7)
        self.assertEqual(json.dumps(r1, sort_keys=True), json.dumps(r2, sort_keys=True))
        self.assertEqual(az.render_markdown(r1), az.render_markdown(r2))

    def test_different_seed_changes_ci_but_not_point_estimate(self):
        rows = self.rows()
        c1 = az.analyze(rows, bootstrap=300, seed=1)["scopes"]["headline"]["comparison"]
        c2 = az.analyze(rows, bootstrap=300, seed=2)["scopes"]["headline"]["comparison"]
        self.assertEqual(c1["diff"], c2["diff"])
        self.assertNotEqual(
            (c1["diff_ci_low"], c1["diff_ci_high"]), (c2["diff_ci_low"], c2["diff_ci_high"])
        )

    def test_point_estimate_matches_hand_computation(self):
        res = az.analyze(self.rows(), bootstrap=200, seed=1)
        cmp_ = res["scopes"]["headline"]["comparison"]
        macro_a = (3 / 4 + 2 / 3 + 1) / 3
        macro_b = (1 / 4 + 2 / 5 + 1 / 2) / 3
        self.assertAlmostEqual(cmp_["macro_a"], macro_a)
        self.assertAlmostEqual(cmp_["macro_b"], macro_b)
        self.assertAlmostEqual(cmp_["diff"], macro_a - macro_b)
        self.assertLessEqual(cmp_["diff_ci_low"], cmp_["diff"] + 1e-9)
        self.assertGreaterEqual(cmp_["diff_ci_high"], cmp_["diff"] - 1e-9)

    def test_ci_is_degenerate_zero_width_when_everything_succeeds(self):
        rows = block(A, "T1", [1, 1, 1]) + block(B, "T1", [1, 1, 1])
        res = run(rows)
        cmp_ = res["scopes"]["headline"]["comparison"]
        self.assertEqual((cmp_["diff_ci_low"], cmp_["diff_ci_high"]), (0.0, 0.0))
        self.assertTrue(cmp_["ci_degenerate"])
        self.assertEqual(cmp_["verdict"], "no resolvable difference")
        self.assertIn("zero width", az.render_markdown(res))

    def test_core_bootstrap_resamples_runs_not_just_tasks(self):
        # One task, A all successes, B half: the CI must have non-zero width
        # (task-only resampling would give a degenerate interval here).
        out = az._bootstrap({A: [[1, 1, 1, 1]], B: [[1, 0, 1, 0]]}, 500, 3)
        self.assertGreater(out["diff"]["ci_high"] - out["diff"]["ci_low"], 0)
        self.assertAlmostEqual(out["diff"]["point"], 0.5)

    def test_percentile_helper(self):
        self.assertAlmostEqual(az.percentile([1, 2, 3, 4], 50), 2.5)
        self.assertAlmostEqual(az.percentile([1, 2, 3, 4], 25), 1.75)
        self.assertEqual(az.percentile([5], 97.5), 5.0)


class DescribeAndEfficiencyTests(unittest.TestCase):
    def test_describe(self):
        d = az.describe([1, 2, 3, 4, None])
        self.assertEqual(d["n"], 4)
        self.assertAlmostEqual(d["median"], 2.5)
        self.assertAlmostEqual(d["mean"], 2.5)
        self.assertAlmostEqual(d["q1"], 1.75)
        self.assertAlmostEqual(d["q3"], 3.25)
        self.assertAlmostEqual(d["iqr"], 1.5)
        self.assertEqual((d["min"], d["max"]), (1.0, 4.0))
        self.assertEqual(az.describe([])["n"], 0)

    def test_efficiency_conditional_vs_unconditional(self):
        rows = [
            make_row(A, "T1", 1, True, wall_s=10.0),
            make_row(A, "T1", 2, True, wall_s=20.0),
            make_row(A, "T1", 3, False, wall_s=100.0),
        ] + block(B, "T1", [1])
        res = run(rows)
        eff = res["efficiency"]["headline"][A]
        self.assertEqual(eff["success_only"]["n_trials"], 2)
        self.assertAlmostEqual(eff["success_only"]["metrics"]["wall_s"]["median"], 15.0)
        self.assertEqual(eff["all_non_excluded"]["n_trials"], 3)
        self.assertAlmostEqual(eff["all_non_excluded"]["metrics"]["wall_s"]["median"], 20.0)
        self.assertAlmostEqual(eff["all_non_excluded"]["metrics"]["tokens_total"]["median"], 1200.0)

    def test_steps_is_tool_calls_total_and_top_level_steps_kept_separately(self):
        row = make_row(A, "T1", 1, True, steps=14)
        row["tool_calls"]["total"] = 9
        res = run([row] + block(B, "T1", [1]))
        metrics = res["cells"][A]["T1"]["metrics"]
        self.assertEqual(metrics["steps"]["median"], 9.0)
        self.assertEqual(metrics["steps_field"]["median"], 14.0)

    def test_null_cost_reduces_n(self):
        rows = [
            make_row(A, "T1", 1, True, est_cost_usd=None),
            make_row(A, "T1", 2, True, est_cost_usd=0.5),
        ] + block(B, "T1", [1])
        res = run(rows)
        s = res["cells"][A]["T1"]["metrics"]["est_cost_usd"]
        self.assertEqual((s["n"], s["median"]), (1, 0.5))

    def test_latency_is_median_of_trial_medians_not_weighted(self):
        lat = lambda n, m, p: {"click": {"n": n, "median": m, "p90": p}}
        rows = [
            make_row(A, "T1", 1, True, action_latency_ms=lat(1000, 10.0, 20.0)),
            make_row(A, "T1", 2, True, action_latency_ms=lat(1, 100.0, 200.0)),
            make_row(A, "T1", 3, True, action_latency_ms=lat(1, 300.0, 400.0)),
            make_row(A, "T1", 4, True, action_latency_ms={}),  # no timed clicks: not counted
        ] + block(B, "T1", [1])
        res = run(rows)
        click = res["latency"]["all_non_excluded"][A]["click"]
        self.assertEqual(click["n_trials"], 3)
        self.assertAlmostEqual(
            click["median_of_trial_medians_ms"], 100.0
        )  # an n-weighted median would be ~10
        self.assertAlmostEqual(click["median_of_trial_p90_ms"], 200.0)
        self.assertEqual(click["total_timed_actions"], 1002)
        self.assertEqual(res["latency"]["all_non_excluded"][A]["drag"]["n_trials"], 0)


class DisturbanceTests(unittest.TestCase):
    def dist(self, **over):
        d = copy.deepcopy(make_row(A, "x", 1, True)["disturbance"])
        d.update(over)
        return d

    def test_filters_and_aggregates(self):
        rows = [
            make_row(A, "T1", 1, True),  # clean
            make_row(
                A,
                "T1",
                2,
                True,
                disturbance=self.dist(front_changes=2, key_loss=1, pointer_max_deviation_px=40.0),
            ),
            make_row(
                A,
                "T1",
                3,
                True,
                disturbance=self.dist(
                    hid_events={"move": 3, "down": 1, "key": 0, "scroll": 0}, clicks_leaked=1
                ),
            ),
            make_row(
                A,
                "T1",
                4,
                True,
                disturbance=self.dist(front_changes=50, human_input_suspected=True),
            ),  # dropped
            make_row(A, "T1", 5, True, disturbance={"available": False}),  # unavailable
            make_row(
                A,
                "T1",
                6,
                False,
                status="infra_error",
                excluded=True,
                excluded_reason="r",
                disturbance=self.dist(front_changes=99),
            ),
        ] + block(B, "T1", [1, 1])
        res = run(rows)
        d = res["disturbance"][A]
        self.assertEqual(d["n_used"], 3)
        self.assertEqual(d["n_dropped_human_input_suspected"], 1)
        self.assertEqual(d["n_unavailable"], 1)
        self.assertAlmostEqual(d["fields"]["front_changes"]["mean"], 2 / 3)
        self.assertEqual(d["fields"]["front_changes"]["max"], 2.0)
        self.assertEqual(d["fields"]["key_loss"]["max"], 1.0)
        self.assertEqual(d["fields"]["clicks_leaked"]["max"], 1.0)
        self.assertEqual(d["fields"]["hid_events_total"]["max"], 4.0)
        self.assertEqual(d["fields"]["hid_events_total"]["sum"], 4.0)
        self.assertEqual(d["fields"]["pointer_max_deviation_px"]["max"], 40.0)
        self.assertEqual(d["any_disturbance"]["count"], 2)
        self.assertAlmostEqual(d["any_disturbance"]["fraction"], 2 / 3)
        self.assertEqual(res["data_quality"]["human_input_suspected_non_excluded"][A], 1)
        self.assertEqual(res["disturbance"][B]["any_disturbance"]["count"], 0)

    def test_scrolls_leaked_is_reported_but_not_part_of_any(self):
        rows = [make_row(A, "T1", 1, True, disturbance=self.dist(scrolls_leaked=4))] + block(
            B, "T1", [1]
        )
        res = run(rows)
        self.assertEqual(res["disturbance"][A]["fields"]["scrolls_leaked"]["max"], 4.0)
        self.assertEqual(res["disturbance"][A]["any_disturbance"]["count"], 0)
        self.assertIn("scrolls_leaked", az.render_markdown(res))

    def test_pointer_episode_alone_counts_as_disturbance(self):
        rows = [
            make_row(A, "T1", 1, True, disturbance=self.dist(pointer_deviation_episodes=1))
        ] + block(B, "T1", [1])
        res = run(rows)
        self.assertEqual(res["disturbance"][A]["any_disturbance"]["count"], 1)

    def test_no_usable_trials_is_reported_not_crashed(self):
        rows = [make_row(A, "T1", 1, True, disturbance={"available": False})] + block(B, "T1", [1])
        res = run(rows)
        self.assertEqual(res["disturbance"][A]["n_used"], 0)
        self.assertIsNone(res["disturbance"][A]["any_disturbance"]["fraction"])
        az.render_markdown(res)


class LoadingTests(unittest.TestCase):
    def write(self, lines):
        handle = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False, encoding="utf-8")
        handle.write("\n".join(lines) + "\n")
        handle.close()
        self.addCleanup(lambda: Path(handle.name).unlink(missing_ok=True))
        return handle.name

    def test_round_trip_and_blank_lines(self):
        rows = block(A, "T1", [1, 0]) + block(B, "T1", [1, 1])
        path = self.write(
            [json.dumps(rows[0]), "", json.dumps(rows[1]), json.dumps(rows[2]), json.dumps(rows[3])]
        )
        loaded = az.load_rows(path)
        self.assertEqual(len(loaded), 4)

    def test_invalid_json_names_line(self):
        good = json.dumps(make_row(A, "T1", 1, True))
        path = self.write(
            [good, good.replace("T1", "T2").replace('"trial_id": "', '"trial_id": "z'), "{not json"]
        )
        with self.assertRaises(az.TrialFormatError) as ctx:
            az.load_rows(path)
        self.assertIn("line 3", str(ctx.exception))
        self.assertIn("invalid JSON", str(ctx.exception))

    def test_schema_violation_names_line_and_field(self):
        good = make_row(A, "T1", 1, True)
        bad = make_row(B, "T1", 1, True, wall_s="fast")
        path = self.write([json.dumps(good), json.dumps(bad)])
        with self.assertRaises(az.TrialFormatError) as ctx:
            az.load_rows(path)
        msg = str(ctx.exception)
        self.assertIn("line 2", msg)
        self.assertIn("wall_s", msg)

    def test_missing_field_wrong_arm_and_wrong_schema(self):
        cases = {
            "task": lambda r: r.pop("task"),
            "arm": lambda r: r.update(arm="someone-else"),
            "schema": lambda r: r.update(schema="cdb-pilot-trial/2"),
            "passed": lambda r: r.update(passed="yes"),
            "status": lambda r: r.update(status="weird"),
            "disturbance.front_changes": lambda r: r["disturbance"].pop("front_changes"),
        }
        for field_name, mutate in cases.items():
            with self.subTest(field=field_name):
                row = make_row(A, "T1", 1, True)
                mutate(row)
                path = self.write([json.dumps(make_row(B, "T1", 1, True)), json.dumps(row)])
                with self.assertRaises(az.TrialFormatError) as ctx:
                    az.load_rows(path)
                self.assertIn("line 2", str(ctx.exception))
                self.assertIn(field_name, str(ctx.exception))

    def test_non_object_line(self):
        path = self.write(["[1, 2, 3]"])
        with self.assertRaises(az.TrialFormatError) as ctx:
            az.load_rows(path)
        self.assertIn("line 1", str(ctx.exception))

    def test_duplicate_trial_id_names_both_lines(self):
        row = json.dumps(make_row(A, "T1", 1, True))
        path = self.write([row, row])
        with self.assertRaises(az.TrialFormatError) as ctx:
            az.load_rows(path)
        self.assertIn("line 2", str(ctx.exception))
        self.assertIn("line 1", str(ctx.exception))

    def test_analyze_rejects_malformed_rows_by_index(self):
        bad = make_row(A, "T1", 1, True)
        del bad["arm"]
        with self.assertRaises(az.TrialFormatError) as ctx:
            az.analyze([make_row(B, "T1", 1, True), bad], bootstrap=10)
        self.assertIn("row 2", str(ctx.exception))

    def test_empty_input_is_an_error(self):
        with self.assertRaises(ValueError):
            az.analyze([], bootstrap=10)


class CliTests(unittest.TestCase):
    def test_cli_writes_markdown_and_json_and_reports_errors(self):
        rows = (
            block(A, "T1", [1, 1, 0])
            + block(B, "T1", [0, 1, 0])
            + block(A, "T2", [1, 1, 1], dimension_tags=["hover"])
            + block(B, "T2", [1, 1, 1], dimension_tags=["hover"])
        )
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            src = tmp_path / "r.jsonl"
            src.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
            out_md, out_json = tmp_path / "o.md", tmp_path / "o.json"
            code = az.main(
                [
                    str(src),
                    "--out-md",
                    str(out_md),
                    "--out-json",
                    str(out_json),
                    "--bootstrap",
                    "200",
                    "--seed",
                    "5",
                    "--pass-k",
                    "2",
                ]
            )
            self.assertEqual(code, 0)
            md = out_md.read_text(encoding="utf-8")
            data = json.loads(out_json.read_text(encoding="utf-8"))
            self.assertIn("## 2. Headline: task-macro success", md)
            self.assertEqual(data["parameters"]["bootstrap"], 200)
            self.assertEqual(data["parameters"]["pass_k"], 2)
            self.assertEqual(data["scopes"]["headline"]["tasks"], ["T1"])
            # same seed, same output
            out_md2 = tmp_path / "o2.md"
            az.main(
                [
                    str(src),
                    "--out-md",
                    str(out_md2),
                    "--bootstrap",
                    "200",
                    "--seed",
                    "5",
                    "--pass-k",
                    "2",
                ]
            )
            self.assertEqual(md, out_md2.read_text(encoding="utf-8"))

            src.write_text(json.dumps(rows[0]) + "\nnot-json\n", encoding="utf-8")
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                code = az.main([str(src)])
            self.assertEqual(code, 2)
            self.assertIn("line 2", err.getvalue())

    def test_cli_rejects_bad_parameters(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "r.jsonl"
            src.write_text(json.dumps(make_row(A, "T1", 1, True)) + "\n", encoding="utf-8")
            for argv in (["--bootstrap", "0"], ["--pass-k", "0"]):
                err = io.StringIO()
                with contextlib.redirect_stderr(err):
                    self.assertEqual(az.main([str(src)] + argv), 2)
                self.assertIn("error", err.getvalue())

    def test_markdown_has_no_non_ascii_and_has_github_tables(self):
        rows = block(A, "T1", [1, 0, 1]) + block(B, "T1", [0, 0, 1])
        md = az.render_markdown(run(rows))
        self.assertTrue(md.isascii())
        self.assertIn("| --- |", md)
        self.assertNotIn("nan", md.lower().replace("n/a", ""))


if __name__ == "__main__":
    unittest.main()
