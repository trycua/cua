"""The perf guard's own logic (no VM): regressions fail, noise does not."""

import unittest

import check

BASE = {
    "ok": 8,
    "parallel": 4,
    "groups_split": [],
    "fork_ms_p50": 1000,
    "fork_ms_p95": 2000,
    "first_ready_ms": 1500,
    "total_ms": 5000,
}


def result(**over):
    r = dict(BASE, failed=0, failures=[], peak_parallel={"local": 4})
    r.update(over)
    return r


class GuardTest(unittest.TestCase):
    def test_within_the_factor_passes(self):
        self.assertEqual(check.check(result(fork_ms_p50=1490, total_ms=7400), BASE, 1.5), [])

    def test_a_slow_fork_p50_fails(self):
        p = check.check(result(fork_ms_p50=1600), BASE, 1.5)
        self.assertEqual(len(p), 1)
        self.assertIn("fork_ms_p50 1600 ms", p[0])

    def test_failures_split_groups_and_parallelism_fail(self):
        p = check.check(
            result(failed=1, failures=["host_failed"], ok=7, groups_split=["w#0"], peak_parallel={"local": 5}),
            BASE,
            1.5,
        )
        text = "\n".join(p)
        for want in ("1 fork(s) failed", "7 ready forks", "groups split", "started 5 forks at once"):
            self.assertIn(want, text)

    def test_the_committed_baseline_has_its_profiles(self):
        import json
        from pathlib import Path

        b = json.loads(Path(check.__file__).with_name("baseline.json").read_text())
        self.assertIn("linux-kvm-gh", b["profiles"])
        for prof in b["profiles"].values():
            for key in check.TIMED:
                self.assertIsInstance(prof[key], int, key)


if __name__ == "__main__":
    unittest.main()
