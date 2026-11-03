"""Fixtures for ci_cache_audit.py (the Rust cache cost audit)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ci_cache_audit as audit  # noqa: E402


class CargoSecondsTests(unittest.TestCase):
    def test_sums_every_finished_line_and_ignores_ansi(self):
        log = (
            "\x1b[1m\x1b[32m    Finished\x1b[0m `dev` profile [unoptimized] target(s) in 1m 23s\n"
            "noise\n"
            "    Finished `release` profile [optimized] target(s) in 45.5s\n"
        )
        self.assertAlmostEqual(audit.cargo_seconds(log), 83 + 45.5)

    def test_no_cargo_output_is_zero(self):
        self.assertEqual(audit.cargo_seconds("nothing compiled here\n"), 0.0)


class VerdictTests(unittest.TestCase):
    def test_long_compile_is_kept(self):
        benefit = audit.estimate(30 * 60)  # a 30 minute cold build
        self.assertEqual(audit.verdict(benefit, 34, 19, 0.1, samples=6, min_runs=3), "keep")

    def test_short_compile_is_dropped(self):
        benefit = audit.estimate(40)  # 40 s cold: about 19 s saved
        self.assertEqual(audit.verdict(benefit, 14, 0, 0.0, samples=6, min_runs=3), "drop?")

    def test_restore_that_eats_the_saving_is_dropped(self):
        benefit = audit.estimate(150)  # about 70 s saved, but the restore takes 60 s
        self.assertEqual(audit.verdict(benefit, 60, 0, 0.0, samples=6, min_runs=3), "drop?")

    def test_too_few_samples_is_not_a_verdict(self):
        self.assertEqual(audit.verdict(900, 10, 10, 0.5, samples=2, min_runs=3), "few runs")

    def test_sccache_estimate_matches_the_measured_warm_saving(self):
        # e2b-bindings, cold run: 1968 requests executed at 1.523 s on 3.4
        # effective cores; a warm directory saved 14.8 of the job's 32.8 minutes.
        log = (
            "2026-10-24T04:32:38.1Z Compile requests executed          1968\n"
            "2026-10-24T04:32:38.1Z Cache hits                            0\n"
            "2026-10-24T04:32:38.1Z Average compiler                  1.523 s\n"
        )
        cold = audit.sccache_cold_seconds(log)
        predicted = audit.estimate(cold, from_sccache=True)
        measured = 14.8 * 60
        self.assertLess(abs(predicted - measured) / measured, 0.25)

    def test_a_run_that_mostly_hit_says_nothing_about_a_cold_build(self):
        log = (
            "Compile requests executed          1968\n"
            "Cache hits                         1955\n"
            "Average compiler                  1.631 s\n"
        )
        self.assertIsNone(audit.sccache_cold_seconds(log))

    def test_cargo_estimate_is_a_screen_that_runs_high(self):
        # The same job's cargo wall times sum to 47.3 minutes (some invocations
        # overlap): the estimate lands above the measured saving, within 50%.
        measured = 14.8 * 60
        predicted = audit.estimate(47.3 * 60)
        self.assertGreater(predicted, measured)
        self.assertLess((predicted - measured) / measured, 0.5)


class WarmOnlyTests(unittest.TestCase):
    WARM_LOG = (
        "2026-10-24T05:00:00.1Z Compile requests executed          1968\n"
        "2026-10-24T05:00:00.1Z Cache hits                         1955\n"
        "2026-10-24T05:00:00.1Z Average compiler                  1.631 s\n"
        "    Finished `dev` profile [unoptimized] target(s) in 41.0s\n"
    )
    COLD_LOG = (
        "2026-10-24T04:00:00.1Z Compile requests executed          1968\n"
        "2026-10-24T04:00:00.1Z Cache hits                            0\n"
        "2026-10-24T04:00:00.1Z Average compiler                  1.523 s\n"
        "    Finished `dev` profile [unoptimized] target(s) in 1500.0s\n"
    )

    def test_a_warm_run_is_not_a_cold_compile(self):
        profile = audit.log_profile(self.WARM_LOG)
        self.assertTrue(profile["warm"])
        self.assertIsNone(profile["sccache_cold"])

    def test_all_warm_logs_give_no_cold_evidence_and_no_drop(self):
        cold, basis = audit.combine([audit.log_profile(self.WARM_LOG)] * 3)
        self.assertEqual(basis, "warm-only")
        self.assertEqual(
            audit.verdict(audit.estimate(cold), 5, 0, 0.0, samples=12, min_runs=3, basis=basis),
            "insufficient cold evidence",
        )

    def test_one_cold_log_among_warm_ones_is_enough(self):
        cold, basis = audit.combine([audit.log_profile(self.WARM_LOG), audit.log_profile(self.COLD_LOG)])
        self.assertEqual(basis, "sccache")
        self.assertGreater(cold, 800)

    def test_a_swatinem_restore_is_warm_but_a_miss_is_cold(self):
        warm = audit.log_profile("Cache restored from key: v0-rust-x\n    Finished `dev` profile target(s) in 20.0s\n")
        miss = audit.log_profile("No cache found.\n    Finished `dev` profile target(s) in 90.0s\n")
        self.assertTrue(warm["warm"])
        self.assertFalse(miss["warm"])
        self.assertEqual(audit.combine([warm, miss]), (90.0, "cargo"))
        self.assertEqual(audit.combine([warm]), (0.0, "warm-only"))

    def test_no_logs_is_not_warm_only(self):
        self.assertEqual(audit.combine([]), (0.0, "cargo"))


if __name__ == "__main__":
    unittest.main()
