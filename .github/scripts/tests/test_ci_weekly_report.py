"""ci_weekly_report.py and flaky/timings.py (CUA-3558, cut #12)."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "flaky"))
import ci_weekly_report as r  # noqa: E402
import timings  # noqa: E402


def run(name, start, end, conclusion="success", path=None):
    return {"name": name, "path": path or f".github/workflows/{name}.yml@refs/pull/1/merge",
            "started": f"2026-11-02T{start}:00Z", "updated": f"2026-11-02T{end}:00Z", "conclusion": conclusion}


def pr(number=1, ref="feature/x", created="2026-11-02T09:00:00Z", merged="2026-11-02T11:00:00Z", signoff=None):
    ctx = [{"context": "local-ci", "createdAt": signoff}] if signoff else []
    return {"number": number, "createdAt": created, "mergedAt": merged, "headRefName": ref,
            "commits": {"nodes": [{"commit": {"oid": "abc", "status": {"contexts": ctx}}}]}}


class PrMetricsTests(unittest.TestCase):
    def test_gate_that_waits_after_the_last_check(self):
        m = r.pr_metrics(pr(), [run("a", "10:00", "10:30"), run("CI: gate", "10:00", "10:40")])
        self.assertEqual(m["push_to_green"], 40)
        self.assertEqual(m["tail"], 10)
        self.assertFalse(m["tail_signoff"])
        self.assertEqual(m["merge_wait"], 20)

    def test_tail_is_blamed_on_the_signoff_when_it_lands_inside_it(self):
        m = r.pr_metrics(pr(signoff="2026-11-02T10:54:00Z"), [run("a", "10:00", "10:30"), run("CI: gate", "10:00", "10:55")])
        self.assertTrue(m["tail_signoff"])

    def test_a_gate_rerun_after_a_label_is_not_a_tail(self):
        m = r.pr_metrics(pr(), [run("a", "10:00", "10:30"), run("CI: gate", "10:50", "10:52")])
        self.assertIsNone(m["tail"])
        self.assertEqual(m["push_to_green"], 30)

    def test_runs_the_gate_ignores_and_cancelled_runs_are_not_the_last_check(self):
        runs = [run("a", "10:00", "10:30"), run("CI: gate", "10:00", "10:32"),
                run("CI: CLA", "10:00", "10:31", path=".github/workflows/cla.yml@x"),
                run("b", "10:00", "10:31", conclusion="cancelled")]
        self.assertEqual(r.pr_metrics(pr(), runs)["tail"], 2)

    def test_no_gate_no_metric(self):
        self.assertIsNone(r.pr_metrics(pr(), [run("a", "10:00", "10:30")]))


class ReportTests(unittest.TestCase):
    def test_report_has_every_section(self):
        prs = [r.pr_metrics(pr(), [run("a", "10:00", "10:30"), run("CI: gate", "10:00", "10:32")]),
               r.pr_metrics(pr(number=2, ref="release-please--branches--main"), [run("a", "10:00", "10:20"), run("CI: gate", "10:00", "10:21")])]
        cd = {"CD: x": {"runs": 2, "fail": 1, "durations": [30.0, 50.0]}}
        timing = [{"suite": "pytest-x", "files": [{"file": "tests.test_a", "seconds": 90, "tests": 4}]}]
        text = r.build_report(prs, cd, timing, "2026-10-29")
        for needle in (r.MARKER, "Push to green", "Release PRs", "CD: x", "tests.test_a", "waiting for the `local-ci` sign-off"):
            self.assertIn(needle, text)

    def test_empty_data_does_not_crash(self):
        text = r.build_report([], {}, [], "2026-10-29")
        self.assertIn("no data", text)
        self.assertIn("No `timings-*` artifacts", text)

    def test_slowest_files_use_the_median_across_runs(self):
        reports = [{"suite": "s", "files": [{"file": "f", "seconds": 10}, {"file": "g", "seconds": 1}]},
                   {"suite": "s", "files": [{"file": "f", "seconds": 30}]}]
        rows = r.slowest_files(reports)
        self.assertEqual((rows[0]["file"], rows[0]["median"], rows[0]["max"]), ("f", 20, 30))


class TimingsTests(unittest.TestCase):
    def test_junit_groups_by_file_and_drops_the_test_class(self):
        xml = ('<testsuites><testsuite>'
               '<testcase classname="tests.test_a.TestOne" name="x" time="2.5"/>'
               '<testcase classname="tests.test_a" name="y" time="1.5"/>'
               '<testcase classname="cua-relay::churn" name="z" time="9"/>'
               '</testsuite></testsuites>')
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "j.xml"
            p.write_text(xml)
            rows = timings.junit_files([str(p)])
        self.assertEqual([(x["file"], x["seconds"], x["tests"]) for x in rows],
                         [("cua-relay::churn", 9.0, 1), ("tests.test_a", 4.0, 2)])

    def test_swift_log_gives_one_row_per_test_class(self):
        log = """Test Suite 'FooTests' passed at 2026-11-02 10:00:00.0.
\t Executed 3 tests, with 0 failures (0 unexpected) in 1.250 (1.260) seconds
Test Suite 'Pkg.xctest' passed at 2026-11-02 10:00:01.0.
\t Executed 3 tests, with 0 failures (0 unexpected) in 1.250 (1.260) seconds
Test Suite 'All tests' passed at 2026-11-02 10:00:01.0.
\t Executed 3 tests, with 0 failures (0 unexpected) in 1.250 (1.260) seconds
""".splitlines()
        rows = timings.swift_log_suites(log)
        self.assertEqual([(x["file"], x["seconds"], x["tests"]) for x in rows], [("FooTests", 1.25, 3)])


class WorkflowCallsTimingsByAbsolutePath(unittest.TestCase):
    def test_every_workflow_runs_timings_py_from_the_workspace_root(self):
        """Jobs set `working-directory` (libs/cua-bench, libs/python/cua-sandbox, ...): a relative path is not found there."""
        import re
        workflows = Path(__file__).resolve().parents[2] / "workflows"
        calls = [(f.name, m) for f in workflows.glob("*.yml")
                 for m in re.findall(r"python3\s+(\S*flaky/timings\.py\S*)", f.read_text())]
        self.assertTrue(calls)
        for name, path in calls:
            self.assertEqual(path, '"$GITHUB_WORKSPACE/.github/scripts/flaky/timings.py"', name)


if __name__ == "__main__":
    unittest.main()
