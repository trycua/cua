"""Unit tests of check.py (the fork fan-out perf guard)."""

import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402

BASE = {
    "checkpoint_ms": 1000.0,
    "waves": {
        "8": {"fork_ms_p50": 800.0, "wall_s": 3.0, "private_mb_per_fork": 100.0},
        "64": {"fork_ms_p50": 900.0, "wall_s": 12.0, "private_mb_per_fork": 100.0},
    },
}


def run_of(p50_8=800.0, wall_64=12.0, waves=(8, 64), ck=1000.0):
    out = {"forkable": True, "checkpoint_ms": ck, "waves": []}
    for n in waves:
        out["waves"].append(
            {
                "forks": n,
                "fork_ms_p50": p50_8 if n == 8 else 900.0,
                "wall_s": wall_64 if n == 64 else 3.0,
                "private_mb_per_fork": 100.0,
            }
        )
    return out


class CheckTest(unittest.TestCase):
    def test_same_numbers_pass(self):
        fails, _ = check.compare(run_of(), BASE, 1.5)
        self.assertEqual(fails, [])

    def test_within_factor_passes(self):
        fails, _ = check.compare(run_of(p50_8=1190.0), BASE, 1.5)
        self.assertEqual(fails, [])

    def test_over_factor_fails_naming_the_metric(self):
        fails, _ = check.compare(run_of(p50_8=1300.0, wall_64=20.0), BASE, 1.5)
        self.assertEqual(len(fails), 2)
        self.assertTrue(fails[0].startswith("8 forks fork_ms_p50: 1300.0 > 1200.0"))
        self.assertTrue(fails[1].startswith("64 forks wall_s: 20.0 > 18.0"))

    def test_slack_covers_tiny_baselines(self):
        base = {"waves": {"8": {"fork_ms_p50": 10.0}}}
        run = {"waves": [{"forks": 8, "fork_ms_p50": 100.0}]}
        self.assertEqual(check.compare(run, base, 1.5)[0], [])

    def test_a_missing_wave_fails(self):
        fails, _ = check.compare(run_of(waves=(8,)), BASE, 1.5)
        self.assertEqual(fails, ["wave of 64 forks: not run"])

    def test_an_unmeasured_metric_fails(self):
        run = run_of()
        del run["waves"][0]["private_mb_per_fork"]
        fails, _ = check.compare(run, BASE, 1.5)
        self.assertEqual(fails, ["8 forks private_mb_per_fork: not measured (baseline 100.0)"])

    def test_cli_exit_codes(self):
        with tempfile.TemporaryDirectory() as d:
            r, b = os.path.join(d, "r.json"), os.path.join(d, "b.json")
            json.dump({"factor": 1.5, "hosts": {"h": BASE}}, open(b, "w"))
            here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "check.py")
            json.dump(run_of(), open(r, "w"))
            ok = subprocess.run([sys.executable, here, r, b, "--host", "h"], capture_output=True)
            self.assertEqual(ok.returncode, 0, ok.stdout)
            json.dump(run_of(p50_8=5000.0), open(r, "w"))
            bad = subprocess.run([sys.executable, here, r, b, "--host", "h"], capture_output=True)
            self.assertEqual(bad.returncode, 1)
            unknown = subprocess.run([sys.executable, here, r, b, "--host", "x"], capture_output=True)
            self.assertEqual(unknown.returncode, 2)


if __name__ == "__main__":
    unittest.main()
