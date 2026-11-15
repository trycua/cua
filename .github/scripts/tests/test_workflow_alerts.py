"""Unit tests for .github/scripts/workflow_alerts.py (CUA-2227). Offline: no network, no PyYAML."""

import datetime as dt
import importlib.util
import json
import re
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("workflow_alerts", ROOT / ".github/scripts/workflow_alerts.py")
wa = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wa)

NOW = dt.datetime(2026, 10, 25, 1, 0, tzinfo=dt.timezone.utc)


def run(conclusion="failure", name="Nightly: Cua Driver", event="schedule", branch="main", repo="trycua/cua", **extra):
    return {
        "name": name,
        "conclusion": conclusion,
        "event": event,
        "head_branch": branch,
        "head_repository": {"full_name": repo},
        "head_sha": "0123456789abcdef0123456789abcdef01234567",
        "run_number": 7,
        "html_url": "https://github.com/%s/actions/runs/1" % repo,
        "run_started_at": "2026-10-25T00:07:00Z",
        "created_at": "2026-10-25T00:07:00Z",
        **extra,
    }


class BuildAlert(unittest.TestCase):
    def test_failure_is_a_firing_alert_that_expires(self):
        alert = wa.build_alert(run("failure"), "Nightly: Cua Driver", "trycua/cua", "ci", NOW, 2)
        self.assertEqual(alert["labels"], {
            "alertname": "GitHubWorkflowFailed", "source": "github-actions", "severity": "warning",
            "environment": "ci", "repo": "trycua/cua", "workflow": "Nightly: Cua Driver",
        })
        self.assertEqual(alert["endsAt"], "2026-10-25T03:00:00Z")
        self.assertEqual(alert["startsAt"], "2026-10-25T00:07:00Z")
        self.assertIn("is red", alert["annotations"]["summary"])
        self.assertIn("a job failed", alert["annotations"]["description"])
        self.assertEqual(alert["annotations"]["run_url"], "https://github.com/trycua/cua/actions/runs/1")

    def test_startup_failure_explains_the_missing_log(self):
        alert = wa.build_alert(run("startup_failure"), "Nightly: Cua Driver", "trycua/cua", "ci", NOW, 2)
        self.assertIn("before any job started", alert["annotations"]["description"])

    def test_success_resolves_now(self):
        alert = wa.build_alert(run("success"), "Nightly: Cua Driver", "trycua/cua", "ci", NOW, 2)
        self.assertEqual(alert["endsAt"], "2026-10-25T01:00:00Z")
        self.assertIn("green again", alert["annotations"]["summary"])

    def test_other_conclusions_change_nothing(self):
        for conclusion in ("cancelled", "skipped", "neutral", "stale", "action_required", None):
            self.assertIsNone(wa.build_alert(run(conclusion), "w", "trycua/cua", "ci", NOW, 2), conclusion)

    def test_fingerprint_labels_ignore_the_run(self):
        a = wa.build_alert(run("failure", run_number=1), "w", "trycua/cua", "ci", NOW, 2)
        b = wa.build_alert(run("success", run_number=2), "w", "trycua/cua", "ci", NOW, 2)
        self.assertEqual(a["labels"], b["labels"])

    def test_runbook_is_optional(self):
        self.assertNotIn("runbook", wa.build_alert(run(), "w", "r/r", "ci", NOW, 2)["annotations"])
        self.assertEqual(wa.build_alert(run(), "w", "r/r", "ci", NOW, 2, "https://x/y")["annotations"]["runbook"], "https://x/y")

    def test_the_workflow_name_is_not_the_run_name(self):
        # A workflow with run-name: reports the run-name as the run's "name".
        alert = wa.build_alert(run(name="Nightly: Cua Driver cua-spaces 709b18d"), "Nightly: Cua Driver", "trycua/cua", "ci", NOW, 2)
        self.assertEqual(alert["labels"]["workflow"], "Nightly: Cua Driver")


class Eligible(unittest.TestCase):
    def test_default_branch_schedule_and_dispatch_count(self):
        for event in ("schedule", "repository_dispatch", "workflow_dispatch"):
            self.assertTrue(wa.eligible(run(event=event), "trycua/cua", "main"), event)

    def test_a_branch_dispatch_does_not(self):
        self.assertFalse(wa.eligible(run(event="workflow_dispatch", branch="agent/x"), "trycua/cua", "main"))

    def test_a_dispatch_on_a_release_tag_counts(self):
        # The release gate is re-run by dispatching the workflow on the tag.
        self.assertTrue(wa.eligible(run(event="workflow_dispatch", branch="cua-spaces-v0.15.0", repo="trycua/cua"), "trycua/cua", "main"))

    def test_a_branch_that_merely_contains_a_version_is_not_a_release_tag(self):
        for branch in ("feature/fix-v1.2-tests", "agent/cua-2227-v1.2", "x/cua-spaces-v0.15.0"):
            self.assertFalse(wa.eligible(run(event="workflow_dispatch", branch=branch), "trycua/cua", "main"), branch)
        for tag in ("cua-spaces-v0.15.0", "cua-driver-rs-v0.38.0", "lume-v0.6.2-nightly.20261015"):
            self.assertTrue(wa.eligible(run(event="workflow_dispatch", branch=tag), "trycua/cua", "main"), tag)

    def test_pushes_to_the_default_branch_and_to_release_tags_count(self):
        self.assertTrue(wa.eligible(run(event="push", branch="main"), "trycua/cua", "main"))
        self.assertTrue(wa.eligible(run(event="push", branch="cua-driver-rs-v0.3.1"), "trycua/cua", "main"))

    def test_a_push_to_a_feature_branch_is_not_the_pipelines_state(self):
        # A branch push that breaks a workflow file shows up as a failed run named after the file path.
        for branch in ("agent/cua-2227-staging-alerts", "dillon/cua-1759-oom-stop-reason", "fix-bug"):
            self.assertFalse(wa.eligible(run(event="push", branch=branch), "trycua/cua", "main"), branch)

    def test_pull_requests_and_forks_never_alert(self):
        self.assertFalse(wa.eligible(run(event="pull_request"), "trycua/cua", "main"))
        self.assertFalse(wa.eligible(run(repo="someone/cua"), "trycua/cua", "main"))


class Watch(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(wa.parse_watch(["a=release", " b ", "", "c="]), {"a": "release", "b": "ci", "c": "ci"})


class Event(unittest.TestCase):
    def dry(self, payload, watch):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "event.json"
            path.write_text(json.dumps(payload))
            with mock.patch.object(wa, "post") as post:
                rc = wa.main(["--repo", "trycua/cua", "--watch", watch, "--event", str(path)])
        return rc, post.call_args[0][0] if post.call_args else None

    def test_failed_watched_run_is_posted(self):
        rc, alerts = self.dry({"workflow_run": run("failure", name="Nightly: Cua Driver cua-spaces 709b18d"),
                               "workflow": {"name": "Nightly: Cua Driver"},
                               "repository": {"full_name": "trycua/cua", "default_branch": "main"}},
                              "Nightly: Cua Driver=ci")
        self.assertEqual(rc, 0)
        self.assertEqual(len(alerts), 1)
        self.assertEqual(alerts[0]["labels"]["environment"], "ci")

    def test_unwatched_or_ineligible_runs_post_nothing(self):
        repo = {"full_name": "trycua/cua", "default_branch": "main"}
        _, alerts = self.dry({"workflow_run": run("failure", name="other"), "workflow": {"name": "other"}, "repository": repo}, "Nightly: Cua Driver")
        self.assertEqual(alerts, [])
        _, alerts = self.dry({"workflow_run": run("failure", event="pull_request"), "workflow": {"name": "Nightly: Cua Driver"}, "repository": repo}, "Nightly: Cua Driver")
        self.assertEqual(alerts, [])


class NewestRun(unittest.TestCase):
    PAYLOAD = {"workflow": {"name": "Nightly: Cua Driver"}, "repository": {"full_name": "trycua/cua", "default_branch": "main"}}

    def post_for(self, run_id, newest_id):
        payload = dict(self.PAYLOAD, workflow_run=run("success", id=run_id, workflow_id=5))
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "event.json"
            path.write_text(json.dumps(payload))
            with mock.patch.object(wa, "latest_run", return_value={"id": newest_id}), mock.patch.object(wa, "post") as post:
                wa.main(["--repo", "trycua/cua", "--watch", "Nightly: Cua Driver", "--event", str(path)])
        return post.call_args[0][0]

    def test_a_late_success_does_not_clear_a_newer_failure(self):
        self.assertEqual(self.post_for(run_id=1, newest_id=2), [])

    def test_the_newest_run_is_reported(self):
        self.assertEqual(len(self.post_for(run_id=2, newest_id=2)), 1)


class Sweep(unittest.TestCase):
    def test_an_eligible_failure_behind_a_page_of_pull_request_runs_is_found(self):
        pr_runs = [run("failure", event="pull_request", created_at="2026-10-25T00:30:00Z")] * 100
        pages = {1: pr_runs, 2: [run("failure", run_number=9, created_at="2026-10-24T22:00:00Z")]}

        def fake(url, token="", data=None, attempts=3):
            page = int(url.rsplit("page=", 1)[1])
            return {"workflow_runs": pages[page]}

        with mock.patch.object(wa, "http_json", fake):
            found = wa.latest_run("trycua/cua", 1, "tok", "main", 14, NOW)
        self.assertEqual(found["run_number"], 9)

    def test_nothing_newer_than_the_age_limit_means_no_alert(self):
        def fake(url, token="", data=None, attempts=3):
            return {"workflow_runs": [run("failure", created_at="2026-09-01T00:00:00Z")]}

        with mock.patch.object(wa, "http_json", fake):
            self.assertIsNone(wa.latest_run("trycua/cua", 1, "tok", "main", 14, NOW))


    def test_latest_eligible_finished_run_of_each_watched_workflow(self):
        def fake(url, token="", data=None, attempts=3):
            self.assertEqual(token, "tok")
            if "/actions/workflows?" in url:
                return {"workflows": [{"id": 1, "name": "a"}, {"id": 2, "name": "b"}, {"id": 3, "name": "unwatched"}]}
            if "/workflows/1/runs" in url:
                return {"workflow_runs": [
                    run("cancelled", name="a"),                                  # skipped: no state
                    run("failure", name="a", event="pull_request"),              # skipped: not eligible
                    run("failure", name="a", run_number=5),                      # the one
                    run("success", name="a", run_number=4),
                ]}
            if "/workflows/2/runs" in url:
                return {"workflow_runs": [run("failure", name="b", created_at="2026-09-01T00:00:00Z")]}  # too old
            self.fail(url)

        with mock.patch.object(wa, "http_json", fake):
            found = wa.latest_runs("trycua/cua", {"a": "ci", "b": "ci", "missing": "ci"}, "tok", "main", 14, NOW)
        self.assertEqual([(name, r["run_number"]) for name, r in found], [("a", 5)])


class WatchListMatchesTheWorkflow(unittest.TestCase):
    """The workflow lists each watched name twice: the workflow_run trigger and --watch."""

    def test_trigger_and_watch_agree(self):
        text = (ROOT / ".github/workflows/workflow-failure-alerts.yml").read_text()
        trigger = re.search(r"workflow_run:\n\s+workflows:\n((?:\s+- .+\n)+)", text).group(1)
        triggered = {line.strip()[2:].strip().strip("\"'") for line in trigger.splitlines()}
        watch = re.search(r"WATCH: \|\n((?: {8,}\S.*\n)+)", text).group(1)
        watched = {line.strip().partition("=")[0] for line in watch.splitlines()}
        self.assertEqual(triggered, watched)
        self.assertTrue(triggered)


if __name__ == "__main__":
    unittest.main()
