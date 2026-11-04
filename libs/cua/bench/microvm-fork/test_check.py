#!/usr/bin/env python3
"""Tests of the perf guard (check.py): it passes within the factor and
fails on a slower p50, failed samples, a missing phase or more memory."""

import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
BASE = {"factor": 1.5, "p50_ms": {"restore": 1000, "fanout": 4000}, "per_fork_mib": 600}


def run(summary, base=BASE):
    with tempfile.TemporaryDirectory() as d:
        s, b = os.path.join(d, "s.json"), os.path.join(d, "b.json")
        json.dump(summary, open(s, "w"))
        json.dump(base, open(b, "w"))
        return subprocess.run(
            [sys.executable, os.path.join(HERE, "check.py"), s, b], capture_output=True, text=True
        )


def phase(p50, n=5, ok=5):
    return {"n": n, "ok": ok, "p50_ms": p50}


class CheckTest(unittest.TestCase):
    def test_within_the_factor_passes(self):
        r = run({"restore": phase(1400), "fanout": phase(3000), "memory": {"per_fork_mib": 700}})
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_a_slower_p50_fails(self):
        r = run({"restore": phase(1600), "fanout": phase(3000), "memory": {"per_fork_mib": 500}})
        self.assertEqual(r.returncode, 1)
        self.assertIn("restore: p50 1600 ms > 1500 ms", r.stderr)

    def test_failed_samples_and_missing_phases_fail(self):
        r = run({"restore": phase(900, n=5, ok=4), "memory": {"per_fork_mib": 500}})
        self.assertEqual(r.returncode, 1)
        self.assertIn("1 of 5 samples failed", r.stderr)
        self.assertIn("fanout: no samples", r.stderr)

    def test_more_memory_per_fork_fails(self):
        r = run({"restore": phase(900), "fanout": phase(3000), "memory": {"per_fork_mib": 901}})
        self.assertEqual(r.returncode, 1)
        self.assertIn("memory: 901 MiB per fork", r.stderr)

    def test_the_committed_baseline_is_well_formed(self):
        base = json.load(open(os.path.join(HERE, "baseline.json")))
        self.assertGreater(base["factor"], 1)
        for k in ("checkpoint", "restore", "fork_running", "fanout"):
            self.assertGreater(base["p50_ms"][k], 0, k)
        self.assertGreater(base["per_fork_mib"], 0)


if __name__ == "__main__":
    unittest.main()
