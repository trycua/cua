"""Run: python3 .github/scripts/tests/test_postmerge_issue.py"""

import argparse
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import postmerge_issue as pm  # noqa: E402

WF = "CI: Cua Spaces for macOS (SwiftUI)"


def a(conclusion="failure", event="push", branch="main"):
    return argparse.Namespace(workflow=WF, conclusion=conclusion, event=event, branch=branch, default_branch="main",
                              sha="0123456789abcdef", run_url="https://example.invalid/run/1", repo="trycua/cua")


def runner(open_issues):
    calls = []

    def run(*args):
        calls.append(args)
        if args[:2] == ("issue", "list"):
            return json.dumps(open_issues)
        return ""
    return run, calls


class PostMerge(unittest.TestCase):
    def test_first_failure_opens_one_issue(self):
        run, calls = runner([])
        self.assertEqual(pm.act(a(), run), "opened")
        create = [c for c in calls if c[:2] == ("issue", "create")]
        self.assertEqual(len(create), 1)
        self.assertIn(pm.title_for(WF), create[0])

    def test_second_failure_comments_instead_of_a_second_issue(self):
        run, calls = runner([{"number": 7, "title": pm.title_for(WF)}])
        self.assertEqual(pm.act(a(), run), "commented")
        self.assertFalse([c for c in calls if c[:2] == ("issue", "create")])
        self.assertTrue([c for c in calls if c[:3] == ("issue", "comment", "7")])

    def test_other_workflows_issue_is_not_reused(self):
        run, _ = runner([{"number": 7, "title": pm.title_for("CI: Cua Spaces")}])
        self.assertEqual(pm.act(a(), run), "opened")

    def test_green_closes_the_open_issue(self):
        run, calls = runner([{"number": 7, "title": pm.title_for(WF)}])
        self.assertEqual(pm.act(a("success"), run), "closed")
        self.assertTrue([c for c in calls if c[:3] == ("issue", "close", "7")])

    def test_green_without_an_issue_does_nothing(self):
        run, calls = runner([])
        self.assertEqual(pm.act(a("success"), run), "nothing")
        self.assertFalse([c for c in calls if c[0] == "issue" and c[1] != "list"])

    def test_pull_requests_and_feature_branches_are_ignored(self):
        run, calls = runner([])
        self.assertEqual(pm.act(a(event="pull_request"), run), "ignored")
        self.assertEqual(pm.act(a(branch="feature"), run), "ignored")
        self.assertEqual(calls, [])

    def test_cancelled_changes_nothing(self):
        run, _ = runner([{"number": 7, "title": pm.title_for(WF)}])
        self.assertEqual(pm.act(a("cancelled"), run), "nothing")


if __name__ == "__main__":
    unittest.main()
