"""Unit tests for .github/scripts/release_pr_automerge.py (CUA-3559). Offline: gh is mocked."""

import datetime as dt
import importlib.util
import sys
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / ".github/scripts"))
SPEC = importlib.util.spec_from_file_location("release_pr_automerge", ROOT / ".github/scripts/release_pr_automerge.py")
rp = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rp)

NOW = dt.datetime(2026, 11, 5, 20, 0, tzinfo=dt.timezone.utc)


def pr(number=1, ref="release-please--branches--main--components--cua-sdk", owner="trycua", labels=(), auto=None, draft=False):
    return {
        "number": number,
        "title": "chore(main): release cua-sdk 1.0.0",
        "headRefName": ref,
        "headRepositoryOwner": {"login": owner},
        "isDraft": draft,
        "labels": [{"name": name} for name in labels],
        "autoMergeRequest": auto,
        "headRefOid": "a" * 40,
    }


def gate(conclusion, completed):
    return {"name": "ci-gate", "conclusion": conclusion, "completed_at": completed}


class WhichPrsCount(unittest.TestCase):
    def test_release_branch_from_this_repo(self):
        self.assertTrue(rp.is_release_pr(pr(), "trycua/cua"))

    def test_fork_branch_with_the_same_name_is_ignored(self):
        self.assertFalse(rp.is_release_pr(pr(owner="someone"), "trycua/cua"))

    def test_ordinary_branch_and_draft_are_ignored(self):
        self.assertFalse(rp.is_release_pr(pr(ref="fix/thing"), "trycua/cua"))
        self.assertFalse(rp.is_release_pr(pr(draft=True), "trycua/cua"))


class Reconcile(unittest.TestCase):
    def test_enables_squash_auto_merge(self):
        with mock.patch.object(rp, "gh") as gh:
            rp.reconcile(pr(7), "trycua/cua")
        gh.assert_called_once_with("pr", "merge", "7", "--repo", "trycua/cua", "--auto", "--squash")

    def test_already_on_is_left_alone(self):
        with mock.patch.object(rp, "gh") as gh:
            rp.reconcile(pr(auto={"mergeMethod": "SQUASH"}), "trycua/cua")
        gh.assert_not_called()

    def test_hold_never_enables_and_switches_off(self):
        with mock.patch.object(rp, "gh") as gh:
            rp.reconcile(pr(labels=["hold"]), "trycua/cua")
            gh.assert_not_called()
            rp.reconcile(pr(5, labels=["hold"], auto={"mergeMethod": "SQUASH"}), "trycua/cua")
        gh.assert_called_once_with("pr", "merge", "5", "--repo", "trycua/cua", "--disable-auto")


class StaleGreen(unittest.TestCase):
    def test_green_for_more_than_six_hours_alerts(self):
        with mock.patch.object(rp, "gh", return_value='{"check_runs": [{"name": "ci-gate", "conclusion": "success", "completed_at": "2026-11-05T13:00:00Z"}]}'):
            alerts = rp.stale_alerts([pr(9)], "trycua/cua", 6, NOW)
        self.assertEqual(len(alerts), 1)
        self.assertEqual(alerts[0]["labels"]["alertname"], "ReleasePRStuck")
        self.assertEqual(alerts[0]["labels"]["pr"], "9")
        self.assertEqual(alerts[0]["startsAt"], "2026-11-05T19:00:00Z")

    def test_recent_red_or_held_does_not_alert(self):
        recent = '{"check_runs": [{"name": "ci-gate", "conclusion": "success", "completed_at": "2026-11-05T18:00:00Z"}]}'
        red = '{"check_runs": [{"name": "ci-gate", "conclusion": "failure", "completed_at": "2026-11-05T01:00:00Z"}]}'
        old = '{"check_runs": [{"name": "ci-gate", "conclusion": "success", "completed_at": "2026-11-05T01:00:00Z"}]}'
        for body in (recent, red):
            with mock.patch.object(rp, "gh", return_value=body):
                self.assertEqual(rp.stale_alerts([pr()], "trycua/cua", 6, NOW), [])
        with mock.patch.object(rp, "gh", return_value=old):
            self.assertEqual(rp.stale_alerts([pr(labels=["hold"])], "trycua/cua", 6, NOW), [])

    def test_latest_gate_run_wins(self):
        runs = [gate("success", "2026-11-05T01:00:00Z"), gate("failure", "2026-11-05T05:00:00Z")]
        self.assertIsNone(rp.gate_green_since(runs))


class BodyGuard(unittest.TestCase):
    def run_guard(self, check_rc, repair_rc, pr_obj=None):
        calls = []

        def fake_run(cmd, **kw):
            calls.append(cmd[2])
            rc = check_rc if cmd[2] == "check" else repair_rc
            return mock.Mock(returncode=rc, stdout="fixed body")

        with mock.patch.object(rp, "gh", return_value="{}") as gh, mock.patch.object(rp.subprocess, "run", fake_run):
            try:
                out = rp.reconcile(pr_obj or pr(3), "trycua/cua", "guard.cjs")
            except rp.BadReleaseBody as bad:
                out = bad
        return out, calls, gh

    def test_good_body_goes_on_to_auto_merge(self):
        out, calls, gh = self.run_guard(0, 1)
        self.assertEqual(calls, ["check"])
        gh.assert_called_with("pr", "merge", "3", "--repo", "trycua/cua", "--auto", "--squash")

    def test_misplaced_heading_is_repaired_then_merged(self):
        out, calls, gh = self.run_guard(1, 0)
        self.assertEqual(calls, ["check", "repair"])
        edits = [c.args for c in gh.call_args_list if c.args[:2] == ("pr", "edit")]
        self.assertEqual(len(edits), 1)
        gh.assert_called_with("pr", "merge", "3", "--repo", "trycua/cua", "--auto", "--squash")

    def test_unrepairable_body_never_auto_merges_and_switches_it_off(self):
        out, calls, gh = self.run_guard(1, 1, pr(3, auto={"mergeMethod": "SQUASH"}))
        self.assertIsInstance(out, rp.BadReleaseBody)
        gh.assert_called_with("pr", "merge", "3", "--repo", "trycua/cua", "--disable-auto")
        self.assertNotIn("--auto", gh.call_args.args)

    def test_unrepairable_body_fails_the_run(self):
        with mock.patch.object(rp, "open_release_prs", return_value=[pr(3)]), mock.patch.object(
            rp, "reconcile", side_effect=rp.BadReleaseBody("bad")
        ):
            self.assertEqual(rp.main(["--repo", "trycua/cua", "--guard", "g.cjs"]), 1)

    def test_hold_skips_the_guard(self):
        with mock.patch.object(rp, "gh"), mock.patch.object(rp, "body_guard") as bg:
            rp.reconcile(pr(labels=["hold"]), "trycua/cua", "guard.cjs")
        bg.assert_not_called()


class Wiring(unittest.TestCase):
    def test_workflow_checks_bodies_on_every_edit(self):
        text = (ROOT / ".github/workflows/release-pr-auto-merge.yml").read_text()
        self.assertIn("synchronize, edited,", text)
        self.assertIn("--guard .github/scripts/release_please_tag_guard.cjs", text)
        self.assertIn("release-please@17.3.0", text)
        self.assertIn("release-please-config.json", text)


    def test_ci_gate_fails_on_hold(self):
        text = (ROOT / ".github/workflows/ci-gate.yml").read_text()
        self.assertIn("contains(github.event.pull_request.labels.*.name, 'hold')", text)
        self.assertIn("labeled, unlabeled", text)


CONFIG = {
    "packages": {
        "libs/cua-driver": {
            "component": "cua-driver-rs",
            "exclude-paths": ["libs/cua-driver/docs", "libs/cua-driver/python"],
        },
        "libs/lume": {"component": "lume"},
    }
}
DRIVER = "release-please--branches--main--components--cua-driver-rs"


def compare(*files):
    import json

    return json.dumps({"ahead_by": len(files), "files": [{"filename": f} for f in files]})


class StaleReleaseBranch(unittest.TestCase):
    """cua-driver-rs 0.41.5: #6834 merged 13 s after #6766, so its changelog missed #6766."""

    def test_new_component_commit_on_main_holds_auto_merge_off(self):
        with mock.patch.object(rp, "gh", return_value=compare("libs/cua-driver/rust/crates/x.rs")) as gh:
            out = rp.reconcile(pr(9, ref=DRIVER, auto={"mergeMethod": "SQUASH"}), "trycua/cua", config=CONFIG)
        self.assertIn("stale", out)
        gh.assert_called_with("pr", "merge", "9", "--repo", "trycua/cua", "--disable-auto")

    def test_stale_pr_never_gets_auto_merge_enabled(self):
        with mock.patch.object(rp, "gh", return_value=compare("libs/cua-driver/rust/a.rs")) as gh:
            rp.reconcile(pr(9, ref=DRIVER), "trycua/cua", config=CONFIG)
        for call in gh.call_args_list:
            self.assertNotIn("--auto", call.args)

    def test_other_component_and_excluded_paths_do_not_count(self):
        with mock.patch.object(rp, "gh", side_effect=[compare("libs/lume/a.swift", "libs/cua-driver/docs/a.md", "libs/cua-driver/python/b.py"), ""]) as gh:
            out = rp.reconcile(pr(9, ref=DRIVER), "trycua/cua", config=CONFIG)
        self.assertIn("auto-merge", out)
        gh.assert_called_with("pr", "merge", "9", "--repo", "trycua/cua", "--auto", "--squash")

    def test_release_tooling_change_counts(self):
        self.assertEqual(rp.stale_files([".github/scripts/release_attribution.py"], ["libs/cua-driver"], []), [".github/scripts/release_attribution.py"])

    def test_up_to_date_branch_is_enabled(self):
        with mock.patch.object(rp, "gh", side_effect=[compare(), ""]) as gh:
            rp.reconcile(pr(9, ref=DRIVER), "trycua/cua", config=CONFIG)
        gh.assert_called_with("pr", "merge", "9", "--repo", "trycua/cua", "--auto", "--squash")

    def test_workflow_sweeps_on_every_main_push(self):
        flow = (ROOT / ".github/workflows/release-pr-auto-merge.yml").read_text()
        self.assertIn("push:\n    branches: [main]", flow)


if __name__ == "__main__":
    unittest.main()


class ReleaseHoldLabel(unittest.TestCase):
    def test_release_hold_label_blocks_enable_and_disables_auto_merge(self):
        with mock.patch.object(rp, "gh") as gh:
            rp.reconcile(pr(labels=["release-hold"]), "trycua/cua")
            gh.assert_not_called()
            rp.reconcile(pr(6, labels=["release-hold"], auto={"mergeMethod": "SQUASH"}), "trycua/cua")
        gh.assert_called_once_with("pr", "merge", "6", "--repo", "trycua/cua", "--disable-auto")

    def test_manually_disabled_auto_merge_without_label_is_re_enabled(self):
        with mock.patch.object(rp, "gh") as gh:
            rp.reconcile(pr(8), "trycua/cua")
        gh.assert_called_once_with("pr", "merge", "8", "--repo", "trycua/cua", "--auto", "--squash")

    def test_ci_gate_fails_on_every_hold_label(self):
        gate = (ROOT / ".github/workflows/ci-gate.yml").read_text()
        for label in rp.HOLD_LABELS:
            self.assertIn("contains(github.event.pull_request.labels.*.name, '%s')" % label, gate)
        self.assertIn("labeled, unlabeled", gate)
        self.assertIn("release-hold", (ROOT / ".github/workflows/release-pr-auto-merge.yml").read_text())
