"""Static security contracts for the review-gated live Jev visual demo."""

from pathlib import Path
import re
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/authorized-live-jev-use-demo.yml"
ORCHESTRATOR = (
    ROOT
    / "libs/cua-driver/rust/crates/cua-driver/tests/authorized_live_jev_use_demo_test.rs"
)
EVIDENCE_README = ROOT / "libs/cua-driver/tests/perception-demo/README.md"


class AuthorizedLiveDemoWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = WORKFLOW.read_text()
        cls.orchestrator = ORCHESTRATOR.read_text()
        cls.evidence_readme = EVIDENCE_README.read_text()
        cls.workflow = yaml.safe_load(cls.text)
        cls.triggers = cls.workflow.get("on", cls.workflow.get(True))
        cls.jobs = cls.workflow["jobs"]

    def test_is_manual_or_callable_with_two_exact_current_pr_heads(self):
        self.assertEqual(set(self.triggers), {"workflow_dispatch", "workflow_call"})
        for trigger in self.triggers.values():
            self.assertEqual(
                set(trigger["inputs"]),
                {"source_sha", "jev_source_sha", "signed_candidate_artifact_id"},
            )
            self.assertTrue(all(value["required"] for value in trigger["inputs"].values()))
        source = self.jobs["source"]["steps"][0]["run"]
        self.assertIn("validate_pr_head 3943", source)
        self.assertIn("validate_pr_head 3916", source)
        self.assertIn('[[ "$requested" == "$head_sha" ]]', source)
        self.assertIn('"$head_repo" == "$GITHUB_REPOSITORY"', source)
        self.assertNotIn("merge-base --is-ancestor", source)
        self.assertNotIn("pull_request_target", self.text)

    def test_artifact_is_source_bound_and_verified_by_actual_driver_trust(self):
        source = self.jobs["source"]["steps"][0]["run"]
        for contract in (
            "actions/artifacts/$CANDIDATE_ARTIFACT_ID",
            ".workflow_run.head_sha",
            "candidate artifact is expired",
            ".conclusion",
        ):
            self.assertIn(contract, source)
        live_text = "\n".join(step.get("run", "") for step in self.jobs["live"]["steps"])
        self.assertIn("extension inspect cua-perception --catalog", live_text)
        self.assertIn("extension install cua-perception --catalog", live_text)
        self.assertIn("extension status cua-perception --self-test --json", live_text)
        self.assertIn('catalog["signature_algorithm"] == measured["signature_algorithm"] == "ed25519"', live_text)
        self.assertIn('status["trust"] == "review-only-publisher-verified"', live_text)
        self.assertIn('digest == files[model["path"]] == model["conversion_sha256"]', live_text)
        self.assertNotIn("--allow-unsigned-local", live_text)
        self.assertNotRegex(live_text, r"--archive\s+[^\n]+--catalog")

    def test_canonical_secret_free_preflights_finish_before_protected_live_job(self):
        self.assertEqual(
            set(self.jobs),
            {"source", "mock-preflight", "windows-preflight", "linux-x11-preflight", "live"},
        )
        preflights = "\n".join(
            str(self.jobs[name])
            for name in ("source", "mock-preflight", "windows-preflight", "linux-x11-preflight")
        )
        self.assertNotIn("environment", preflights)
        self.assertNotIn("TYPESAFE_API_KEY", preflights)
        self.assertEqual(self.jobs["live"]["environment"], "authorized-live-jev-use-demo")
        self.assertEqual(
            set(self.jobs["live"]["needs"]),
            {"source", "mock-preflight", "windows-preflight", "linux-x11-preflight"},
        )
        windows = "\n".join(step.get("run", "") for step in self.jobs["windows-preflight"]["steps"])
        linux = "\n".join(step.get("run", "") for step in self.jobs["linux-x11-preflight"]["steps"])
        self.assertIn("scripts\\ci\\windows\\run-rust-e2e.ps1 -RequireGui", windows)
        self.assertIn("scripts/ci/linux/run-rust-e2e.sh", linux)
        self.assertIn("xvfb-run", linux)

    def test_live_matrix_uses_review_candidates_and_orchestrator_supports_macos(self):
        matrix = self.jobs["live"]["strategy"]["matrix"]["include"]
        self.assertEqual(
            {item["platform"]: item["runner"] for item in matrix},
            {
                "windows": "windows-latest",
                "linux-x11": "ubuntu-latest",
            },
        )
        self.assertEqual(
            {item["platform"]: item["driver"] for item in matrix},
            {"windows": "review-cua-driver.exe", "linux-x11": "review-cua-driver"},
        )
        self.assertIn('target_os = "macos"', self.orchestrator)
        self.assertIn('#[cfg(any(target_os = "linux", target_os = "macos"))]', self.orchestrator)
        shared = self.orchestrator.index('.arg(fixture_path())')
        self.assertGreater(shared, self.orchestrator.index('Command::new("py")'))
        self.assertGreater(shared, self.orchestrator.index('Command::new("python3")'))

    def test_external_chooser_receives_only_the_key_and_windows_system_root(self):
        start = self.orchestrator.index("fn external_choice(")
        end = self.orchestrator.index("fn choose(", start)
        external = self.orchestrator[start:end]
        self.assertIn(".env_clear()", external)
        self.assertIn('.env("TYPESAFE_API_KEY", required("TYPESAFE_API_KEY"))', external)
        self.assertIn('command.env("SYSTEMROOT", system_root)', external)
        self.assertEqual(external.count(".env("), 2)

    def test_review_driver_is_consumed_from_hash_bound_aggregate(self):
        live_text = "\n".join(step.get("run", "") for step in self.jobs["live"]["steps"])
        self.assertIn("review-measurements.json", live_text)
        self.assertIn('measured["review_driver_sha256"]', live_text)
        self.assertIn('measured["review_driver_build_profile"] == "debug-review-trust-root"', live_text)
        self.assertIn("CUA_TEST_DRIVER_BIN", live_text)
        self.assertIn("signed-candidate-checksums.txt", live_text)
        self.assertNotIn("cargo build --manifest-path", live_text)
        self.assertNotIn("target/release/cua-driver", live_text)
        self.assertNotIn("RSA-SHA256", self.evidence_readme)
        self.assertIn("Ed25519", self.evidence_readme)
        self.assertIn("debug review-trust-root", self.evidence_readme)

    def test_only_measured_binary_receives_secret_in_one_bounded_step(self):
        secret_steps = [step for step in self.jobs["live"]["steps"] if "${{ secrets." in str(step)]
        self.assertEqual(len(secret_steps), 1)
        secret = secret_steps[0]
        self.assertEqual(secret["timeout-minutes"], 15)
        self.assertEqual(
            secret["env"], {"LIVE_TYPESAFE_API_KEY": "${{ secrets.TYPESAFE_API_KEY }}"}
        )
        self.assertNotIn("cargo ", secret["run"])
        self.assertIn("CUA_LIVE_TEST_BINARY_SHA256", secret["run"])
        self.assertIn("Remove-Item Env:LIVE_TYPESAFE_API_KEY", secret["run"])
        self.assertIn("Remove-Item Env:TYPESAFE_API_KEY", secret["run"])
        self.assertIn(
            "& $env:CUA_LIVE_TEST_BINARY --ignored --exact authorized_visual_only_demo",
            secret["run"],
        )
        compile_step = next(
            step
            for step in self.jobs["live"]["steps"]
            if step.get("name", "").startswith("Compile and measure")
        )
        self.assertIn("--no-run --message-format=json", compile_step["run"])
        self.assertIn('"authorized_visual_only_demo: test"', compile_step["run"])

    def test_chooser_is_exact_fixed_and_fails_closed_when_absent(self):
        live = self.jobs["live"]
        checkout = next(
            step
            for step in live["steps"]
            if step.get("name") == "Check out the exact reviewed Jev chooser"
        )
        self.assertEqual(checkout["with"]["ref"], "${{ needs.source.outputs.jev_sha }}")
        setup = next(
            step
            for step in live["steps"]
            if step.get("name", "").startswith("Install the locked reviewed chooser")
        )
        self.assertIn("python/choose_action.py", setup["run"])
        self.assertIn("uv sync --frozen --project", setup["run"])
        self.assertIn("reviewed live chooser contract is unavailable", setup["run"])
        secret = next(step for step in live["steps"] if "${{ secrets." in str(step))
        self.assertIn("CUA_JEV_CHOOSER_PROGRAM", secret["run"])
        self.assertIn("CUA_JEV_CHOOSER_SCRIPT", secret["run"])
        self.assertNotIn("Invoke-Expression", self.text)
        self.assertNotIn("bash -c", secret["run"])

    def test_mock_has_no_chooser_or_key(self):
        mock = next(
            step
            for step in self.jobs["live"]["steps"]
            if "deterministic mock" in step.get("name", "")
        )
        self.assertEqual(
            mock["env"],
            {
                "CUA_JEV_MOCK_DEMO": "1",
                "CUA_PERCEPTION_EVIDENCE_DIR": "${{ runner.temp }}/perception-evidence/mock",
            },
        )
        self.assertNotIn("CHOOSER", str(mock))
        self.assertNotIn("TYPESAFE", str(mock))

    def test_only_decoded_schema_validated_redacted_evidence_is_uploaded(self):
        uploads = [
            step for step in self.jobs["live"]["steps"] if "upload-artifact" in step.get("uses", "")
        ]
        self.assertEqual(len(uploads), 1)
        self.assertEqual(uploads[0]["with"]["path"], "${{ runner.temp }}/publish-evidence/")
        validate = next(
            step
            for step in self.jobs["live"]["steps"]
            if step.get("name", "").startswith("Fully decode")
        )["run"]
        self.assertIn("ffmpeg -v error -xerror", validate)
        self.assertIn("Draft202012Validator(schema).validate(manifest)", validate)
        self.assertIn(
            'assert sorted(path.name for path in output.iterdir()) == ["manifest.json", "recording.mp4"]',
            validate,
        )
        self.assertIn('perception["signed_extension_archive_sha256"] == measured["archive_sha256"]', validate)
        self.assertIn('driver = runtime["driver"]', validate)
        self.assertIn('observation = manifest["observation"]', validate)
        self.assertIn('manifest["os"] ==', validate)
        self.assertNotIn("raw-manifest.json", str(uploads[0]))
        self.assertNotIn("timeline.json", str(uploads[0]))

    def test_all_actions_are_full_sha_pinned(self):
        uses = re.findall(r"^\s*-?\s*uses:\s*([^\s#]+)", self.text, re.MULTILINE)
        self.assertTrue(uses)
        for action in uses:
            self.assertRegex(action, r"^[^@]+@[0-9a-f]{40}$")


if __name__ == "__main__":
    unittest.main()
