"""Static contracts for the review-gated visual perception demo preflight."""

from pathlib import Path
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/authorized-live-jev-use-demo.yml"


class AuthorizedLiveDemoWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = WORKFLOW.read_text()
        cls.workflow = yaml.safe_load(cls.text)

    def test_is_manual_only_and_accepts_only_the_current_pr_3943_head(self):
        triggers = self.workflow.get("on", self.workflow.get(True))
        self.assertEqual(set(triggers), {"workflow_dispatch"})
        source = self.workflow["jobs"]["source"]["steps"][0]["run"]
        self.assertIn("^[0-9a-f]{40}$", source)
        self.assertIn("pulls/3943", source)
        self.assertIn('"$REQUESTED_SHA" != "$head_sha"', source)
        self.assertIn('"$head_repo" != "$GITHUB_REPOSITORY"', source)
        self.assertNotIn("merge-base --is-ancestor", source)
        self.assertNotIn("pull_request_target", self.text)

    def test_workflow_stays_secret_free_and_artifact_gates_the_mock_e2e(self):
        jobs = self.workflow["jobs"]
        self.assertEqual(
            set(jobs),
            {"source", "mock-preflight", "windows-preflight", "linux-x11-preflight"},
        )
        self.assertNotIn("environment:", self.text)
        self.assertNotIn("TYPESAFE_API_KEY", self.text)
        self.assertNotIn("${{ secrets.", self.text)
        self.assertIn("actions/upload-artifact@65c4c4a1", self.text)
        upload_steps = [
            step
            for name in ("windows-preflight", "linux-x11-preflight")
            for step in jobs[name]["steps"]
            if "upload-artifact" in step.get("uses", "")
        ]
        self.assertEqual(len(upload_steps), 2)
        for step in upload_steps:
            self.assertEqual(step["if"], "inputs.signed_candidate_artifact_id != ''")
            self.assertIn("recording.mp4", step["with"]["path"])
            self.assertIn("manifest.json", step["with"]["path"])
            self.assertNotIn("raw-manifest.json", step["with"]["path"])
            self.assertNotIn("timeline.json", step["with"]["path"])
        self.assertIn("signed_candidate_artifact_id", self.text)
        self.assertIn("inputs.signed_candidate_artifact_id != ''", self.text)
        self.assertIn("artifact-ids:", self.text)
        self.assertIn("--ignored --exact", self.text)
        self.assertIn("No live Jev/API call is made", self.text)

    def test_platform_preflights_use_canonical_harnesses(self):
        jobs = self.workflow["jobs"]
        windows_steps = "\n".join(
            step.get("run", "") for step in jobs["windows-preflight"]["steps"]
        )
        linux_steps = "\n".join(
            step.get("run", "") for step in jobs["linux-x11-preflight"]["steps"]
        )
        self.assertIn("scripts\\ci\\windows\\run-rust-e2e.ps1 -RequireGui", windows_steps)
        self.assertIn("scripts/ci/linux/run-rust-e2e.sh", linux_steps)
        self.assertIn("xvfb-run", linux_steps)
        self.assertEqual(jobs["windows-preflight"]["env"]["CUA_E2E_INTERNAL_LANE"], "capture")
        self.assertEqual(jobs["linux-x11-preflight"]["env"]["CUA_E2E_INTERNAL_LANE"], "capture")
        for line in self.text.splitlines():
            if "uses:" in line:
                self.assertRegex(line, r"@[0-9a-f]{40}(?:\s|$)")

    def test_orchestration_is_mock_choice_capture_bound_and_has_no_live_client(self):
        rust_test = (
            ROOT
            / "libs/cua-driver/rust/crates/cua-driver/tests/authorized_live_jev_use_demo_test.rs"
        ).read_text()
        self.assertIn('driver.call("parse_visual_regions"', rust_test)
        self.assertIn('"capture_id": choice.capture_id', rust_test)
        self.assertIn('state["selected"] == "send"', rust_test)
        self.assertIn("capture reuse was not refused", rust_test)
        self.assertIn("raw-manifest.json", rust_test)
        self.assertIn("timeline.json", rust_test)
        self.assertIn("manifest.json", rust_test)
        self.assertIn("copy decoded recording evidence", rust_test)
        self.assertNotIn("TYPESAFE_API_KEY", rust_test)
        self.assertNotIn("reqwest", rust_test)
        self.assertNotIn("ureq", rust_test)


if __name__ == "__main__":
    unittest.main()
