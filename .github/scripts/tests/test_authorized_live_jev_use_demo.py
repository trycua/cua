"""Static contracts for the review-gated visual perception demo workflow."""

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

    def test_is_manual_only_and_requires_exact_sha(self):
        triggers = self.workflow.get("on", self.workflow.get(True))
        self.assertEqual(set(triggers), {"workflow_dispatch"})
        source = self.workflow["jobs"]["source"]["steps"][1]["run"]
        self.assertIn("^[0-9a-f]{40}$", source)
        self.assertIn("merge-base --is-ancestor", source)
        self.assertNotIn("pull_request_target", self.text)

    def test_unprotected_preflights_gate_both_live_platforms(self):
        jobs = self.workflow["jobs"]
        self.assertNotIn("environment", jobs["mock-preflight"])
        self.assertNotIn("environment", jobs["windows-preflight"])
        self.assertNotIn("environment", jobs["linux-x11-preflight"])
        expected = {"source", "mock-preflight", "windows-preflight", "linux-x11-preflight"}
        for name in ("live-windows", "live-linux-x11"):
            self.assertEqual(set(jobs[name]["needs"]), expected)
            self.assertEqual(jobs[name]["environment"], "authorized-live-jev-use-demo")

    def test_secret_is_job_scoped_and_uploads_are_review_safe(self):
        self.assertEqual(self.text.count("TYPESAFE_API_KEY: ${{ secrets.TYPESAFE_API_KEY }}"), 2)
        self.assertNotIn("TYPESAFE_API_KEY", self.text.split("jobs:", 1)[0])
        for name in ("live-windows", "live-linux-x11"):
            job = self.workflow["jobs"][name]
            self.assertEqual(job["env"]["TYPESAFE_API_KEY"], "${{ secrets.TYPESAFE_API_KEY }}")
            upload = next(step for step in job["steps"] if step.get("name") == "Upload review evidence")
            paths = upload["with"]["path"]
            self.assertIn("recording.mp4", paths)
            self.assertIn("manifest.json", paths)
            self.assertNotIn("trajectory", paths)
            self.assertNotIn("log", paths.lower())

    def test_live_command_uses_fail_closed_skeleton(self):
        self.assertIn("authorized_visual_only_demo", self.text)
        rust_test = (
            ROOT
            / "libs/cua-driver/rust/crates/cua-driver/tests/authorized_live_jev_use_demo_test.rs"
        ).read_text()
        self.assertIn("reviewed live Jev adapter is not linked", rust_test)
        self.assertIn("no live API request was made", rust_test)
        self.assertNotIn("reqwest", rust_test)
        self.assertNotIn("ureq", rust_test)


if __name__ == "__main__":
    unittest.main()
