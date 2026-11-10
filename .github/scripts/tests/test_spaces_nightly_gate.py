"""Run: python3 .github/scripts/tests/test_spaces_nightly_gate.py (CUA-3503)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import spaces_nightly_gate as gate  # noqa: E402


def run(name, status="completed", conclusion="success", event="push", id=1):
    return {"id": id, "path": f".github/workflows/{name}", "event": event, "status": status, "conclusion": conclusion}


class Verdict(unittest.TestCase):
    def test_all_spaces_ci_green(self):
        runs = [run("ci-cua-spaces.yml"), run("ci-cua-spaces-web.yml"), run("ci-cua-spacesd.yml")]
        self.assertEqual(gate.verdict(runs), ("green", []))

    def test_a_failed_spaces_workflow_is_red(self):
        runs = [run("ci-cua-spaces.yml"), run("ci-cua-spaces-macos.yml", conclusion="failure")]
        self.assertEqual(gate.verdict(runs), ("red", ["ci-cua-spaces-macos.yml"]))

    def test_red_wins_over_pending(self):
        runs = [run("ci-cua-spaces-web.yml", conclusion="timed_out"), run("ci-cua-spaces.yml", status="in_progress", conclusion=None)]
        self.assertEqual(gate.verdict(runs)[0], "red")

    def test_running_is_pending(self):
        runs = [run("ci-cua-spaces.yml"), run("ci-cua-spaces-rust-slice.yml", status="queued", conclusion=None)]
        self.assertEqual(gate.verdict(runs), ("pending", []))

    def test_other_workflows_and_events_do_not_count(self):
        runs = [
            run("ci-rust.yml", conclusion="failure"),
            run("cd-cua-spaces-build.yml", conclusion="failure"),
            run("ci-cua-spaces.yml", event="pull_request", conclusion="failure"),
        ]
        self.assertEqual(gate.verdict(runs), ("none", []))

    def test_cancelled_is_not_red(self):
        self.assertEqual(gate.verdict([run("ci-cua-spaces.yml", conclusion="cancelled")]), ("green", []))

    def test_newest_run_of_a_workflow_wins(self):
        runs = [run("ci-cua-spaces.yml", conclusion="failure", id=1), run("ci-cua-spaces.yml", id=2)]
        self.assertEqual(gate.verdict(runs), ("green", []))


class CommitState(unittest.TestCase):
    def test_walks_back_to_the_newest_commit_that_ran_spaces_ci(self):
        history = {"c3": [run("ci-rust.yml")], "c2": [run("ci-cua-spaces.yml", conclusion="failure")], "c1": [run("ci-cua-spaces.yml")]}
        state, failed, at = gate.commit_state("o/r", "c3", fetch=lambda _r, c: history[c], parents=["c3", "c2", "c1"])
        self.assertEqual((state, failed, at), ("red", ["ci-cua-spaces.yml"], "c2"))

    def test_head_with_its_own_runs(self):
        history = {"c2": [run("ci-cua-spaces.yml")], "c1": [run("ci-cua-spaces.yml", conclusion="failure")]}
        self.assertEqual(gate.commit_state("o/r", "c2", fetch=lambda _r, c: history[c], parents=["c2", "c1"])[0], "green")

    def test_nothing_found_is_pending(self):
        self.assertEqual(gate.commit_state("o/r", "c1", fetch=lambda _r, _c: [], parents=["c1"])[0], "pending")


if __name__ == "__main__":
    unittest.main()
