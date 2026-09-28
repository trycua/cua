"""Static security contracts for the review-gated candidate Jev visual demo."""

from pathlib import Path
import re
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/authorized-live-jev-use-demo.yml"
ORCHESTRATOR = (
    ROOT / "libs/cua-driver/rust/crates/cua-driver-e2e/tests/authorized_live_jev_use_demo_test.rs"
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

    def candidate_steps(self):
        return self.jobs["candidate"]["steps"]

    def candidate_text(self):
        return "\n".join(step.get("run", "") for step in self.candidate_steps())

    def test_workflow_is_credential_free(self):
        self.assertNotIn("TYPESAFE_API_KEY", self.text)
        self.assertNotIn("${{ secrets.", self.text)
        self.assertNotIn("run_live", self.text)
        self.assertNotIn("RUN_LIVE_ACKNOWLEDGED", self.text)
        self.assertNotIn("jev-use-live", self.text)
        for job_name in ("source", "mock-preflight"):
            self.assertNotIn("environment", self.jobs[job_name])
            self.assertNotIn("TYPESAFE_API_KEY", str(self.jobs[job_name]))

    def test_is_manual_or_callable_with_open_source_head_and_merged_jev_commit(self):
        self.assertEqual(set(self.triggers), {"workflow_dispatch", "workflow_call"})
        dispatch_inputs = self.triggers["workflow_dispatch"]["inputs"]
        callable_inputs = self.triggers["workflow_call"]["inputs"]
        self.assertEqual(dispatch_inputs, callable_inputs)
        self.assertNotRegex(self.text, r"(?m)^\s*inputs:\s*[&*]")
        for trigger in self.triggers.values():
            self.assertEqual(
                set(trigger["inputs"]),
                {
                    "source_pr_number",
                    "source_sha",
                    "jev_pr_number",
                    "jev_source_sha",
                    "signed_candidate_artifact_id",
                    "signed_candidate_run_id",
                    "windows_e2e_run_id",
                    "linux_e2e_run_id",
                },
            )
            self.assertTrue(all(value["required"] for value in trigger["inputs"].values()))
        source = self.jobs["source"]["steps"][0]["run"]
        self.assertNotIn("RUN_LIVE_ACKNOWLEDGED", source)
        self.assertIn('validate_pr_head "$SOURCE_PR_NUMBER"', source)
        self.assertIn('validate_merged_pr "$JEV_PR_NUMBER"', source)
        self.assertNotIn("CANONICAL_JEV_MERGE_SHA", self.text)
        self.assertIn('[[ "$requested" == "$head_sha" ]]', source)
        self.assertIn('"$head_repo" == "$GITHUB_REPOSITORY"', source)
        self.assertIn('[[ "$state" == closed && "$merged" == true', source)
        self.assertIn('"$requested" == "$merge_sha"', source)
        self.assertNotIn("merge-base --is-ancestor", source)
        self.assertNotIn("pull_request_target", self.text)
        self.assertIn('[[ "$run_id" == "$CANDIDATE_RUN_ID" ]]', source)
        self.assertIn("STAGING-cua-perception-review-candidates-$REQUESTED_SHA", source)
        self.assertIn(".github/workflows/review-cua-perception-candidates.yml", source)
        self.assertIn('.event <<<"$run_json")" == pull_request', source)
        self.assertIn('[[ "$EVENT_ACTION" == labeled', source)
        self.assertIn('"$EVENT_PR_NUMBER" == "$SOURCE_PR_NUMBER"', source)
        self.assertIn('"$EVENT_HEAD_SHA" == "$REQUESTED_SHA"', source)
        self.assertIn('"$EVENT_LABEL" == cua-perception-live-review', source)
        self.assertIn('"$run_id" == "$GITHUB_RUN_ID"', source)
        self.assertIn('"$GITHUB_EVENT_NAME" == workflow_dispatch', source)
        self.assertIn('.status <<<"$run_json")" == in_progress', source)
        self.assertIn('.status <<<"$run_json")" == completed', source)
        download = next(
            step
            for step in self.candidate_steps()
            if "download-artifact" in step.get("uses", "")
        )
        self.assertEqual(download["with"]["run-id"], "${{ needs.source.outputs.candidate_run_id }}")
        self.assertIn("d3f86a106a0bac45b974a628896c90dbdf5c8093", download["uses"])

    def test_artifact_is_source_bound_and_verified_by_actual_driver_trust(self):
        source = self.jobs["source"]["steps"][0]["run"]
        for contract in (
            "actions/artifacts/$CANDIDATE_ARTIFACT_ID",
            ".workflow_run.head_sha",
            "candidate artifact is expired",
            ".conclusion",
        ):
            self.assertIn(contract, source)
        candidate_text = self.candidate_text()
        self.assertIn("extension inspect cua-perception --catalog", candidate_text)
        self.assertIn("extension install cua-perception --catalog", candidate_text)
        self.assertIn("extension status cua-perception --self-test --json", candidate_text)
        self.assertIn(
            'catalog["signature_algorithm"] == measured["signature_algorithm"] == "ed25519"',
            candidate_text,
        )
        self.assertIn('status["trust"] == "review-only-publisher-verified"', candidate_text)
        self.assertIn('digest == files[model["path"]] == model["conversion_sha256"]', candidate_text)
        self.assertNotIn("--allow-unsigned-local", candidate_text)
        self.assertNotRegex(candidate_text, r"--archive\s+[^\n]+--catalog")

    def test_canonical_runs_are_attested_before_protected_candidate_job(self):
        self.assertEqual(
            set(self.jobs),
            {"source", "mock-preflight", "candidate"},
        )
        preflights = "\n".join(str(self.jobs[name]) for name in ("source", "mock-preflight"))
        self.assertNotIn("environment", preflights)
        self.assertNotIn("TYPESAFE_API_KEY", preflights)
        self.assertEqual(self.jobs["candidate"]["environment"], "authorized-live-jev-use-demo")
        self.assertEqual(self.jobs["candidate"]["env"]["CUA_E2E_UNRESTRICTED_GUI"], "1")
        self.assertEqual(
            set(self.jobs["candidate"]["needs"]),
            {"source", "mock-preflight"},
        )
        source = self.jobs["source"]["steps"][0]["run"]
        mock = "\n".join(step.get("run", "") for step in self.jobs["mock-preflight"]["steps"])
        self.assertIn("python3-tk", mock)
        self.assertIn("libx11-dev", mock)
        self.assertIn("libwebkit2gtk-4.1-dev", mock)
        self.assertLess(mock.index("python3-tk"), mock.index("test_fixture.py"))
        self.assertIn(".github/workflows/e2e-rust-windows.yml", source)
        self.assertIn("rust-windows-e2e-certification", source)
        self.assertIn(".github/workflows/e2e-rust-linux.yml", source)
        self.assertIn("rust-linux-e2e-certification", source)
        self.assertNotIn("run-rust-e2e", self.text)

    def test_candidate_matrix_uses_review_candidates_and_orchestrator_supports_macos(self):
        matrix = self.jobs["candidate"]["strategy"]["matrix"]["include"]
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
        shared = self.orchestrator.index(".arg(fixture_path())")
        self.assertGreater(shared, self.orchestrator.index('Command::new("py")'))
        self.assertGreater(shared, self.orchestrator.index('Command::new("python3")'))

    def test_candidate_paths_use_private_windows_extension_home_and_runner_temp_evidence(self):
        candidate_env = self.jobs["candidate"]["env"]
        self.assertNotIn("CUA_PERCEPTION_EXTENSION_HOME", candidate_env)
        self.assertNotIn("CUA_PERCEPTION_EVIDENCE_DIR", candidate_env)
        self.assertNotIn("CUA_E2E_RECORDINGS_ROOT", candidate_env)
        steps = self.candidate_steps()
        configure = next(
            step for step in steps if step.get("name") == "Configure temporary evidence paths"
        )
        self.assertIn(
            '$extensionHomeName = "cua-perception-extension-home-$($env:GITHUB_RUN_ID)-$($env:GITHUB_RUN_ATTEMPT)"',
            configure["run"],
        )
        self.assertIn("$userProfile = [IO.Path]::GetFullPath($env:USERPROFILE)", configure["run"])
        self.assertIn(
            "$userProfileItem.Attributes -band [IO.FileAttributes]::ReparsePoint", configure["run"]
        )
        self.assertIn(
            "[IO.Path]::GetDirectoryName($extensionHome) -ne $userProfile", configure["run"]
        )
        self.assertIn(
            "if (Test-Path -LiteralPath $extensionHome) { throw 'Windows extension home must be fresh' }",
            configure["run"],
        )
        self.assertIn("New-Item -ItemType Directory -Path $extensionHome", configure["run"])
        self.assertIn(
            "$extensionHomeItem.Attributes -band [IO.FileAttributes]::ReparsePoint",
            configure["run"],
        )
        self.assertNotIn("LOCALAPPDATA", configure["run"])
        self.assertIn(
            "$extensionHome = Join-Path $env:RUNNER_TEMP 'cua-perception-extension-home/mock'",
            configure["run"],
        )
        self.assertIn(
            '"CUA_PERCEPTION_EXTENSION_HOME=$extensionHome" >> $env:GITHUB_ENV', configure["run"]
        )
        self.assertIn('"CUA_DRIVER_RS_HOME=$extensionHome" >> $env:GITHUB_ENV', configure["run"])
        self.assertIn(
            "CUA_E2E_RECORDINGS_ROOT=$(Join-Path $env:RUNNER_TEMP 'cua-perception-recordings')",
            configure["run"],
        )
        self.assertIn(
            "CUA_PERCEPTION_EVIDENCE_DIR=$(Join-Path $env:RUNNER_TEMP 'cua-perception-evidence/mock')",
            configure["run"],
        )
        self.assertNotIn("if", configure)
        mock = next(step for step in steps if "deterministic mock" in step.get("name", ""))
        self.assertLess(steps.index(configure), steps.index(mock))
        self.assertEqual(
            {item["platform"] for item in self.jobs["candidate"]["strategy"]["matrix"]["include"]},
            {"windows", "linux-x11"},
        )
        self.assertNotRegex(
            self.text,
            r"CUA_PERCEPTION_EXTENSION_HOME[^\n]*github\.workspace",
        )
        self.assertNotRegex(
            self.text,
            r"CUA_E2E_RECORDINGS_ROOT[^\n]*github\.workspace",
        )
        self.assertNotIn("github.workspace", configure["run"])
        self.assertFalse(any("${{ runner." in value for value in candidate_env.values()))

    def test_candidate_linux_desktop_persists_and_is_rechecked_before_demos(self):
        steps = self.candidate_steps()
        prepare = next(step for step in steps if step.get("name") == "Prepare Linux X11 desktop")
        self.assertEqual(prepare["if"], "runner.os == 'Linux'")
        self.assertRegex(prepare["run"], r"apt-get install[^\n]*\bx11-utils\b")
        self.assertLess(prepare["run"].index("x11-utils"), prepare["run"].index("xdpyinfo"))
        for daemon in ("Xvfb", "dbus-daemon", "openbox", "picom"):
            self.assertRegex(
                prepare["run"],
                rf"nohup env -u RUNNER_TRACKING_ID[^\n]*(?:\n[^\n]*)?\b{re.escape(daemon)}\b",
            )
        self.assertIn("cua-linux-desktop.pids", prepare["run"])
        self.assertIn("xprop -root _NET_SUPPORTING_WM_CHECK _NET_CLIENT_LIST", prepare["run"])
        self.assertIn("^_NET_SUPPORTING_WM_CHECK(WINDOW): window id # 0x", prepare["run"])
        self.assertIn("^_NET_CLIENT_LIST(WINDOW):", prepare["run"])
        self.assertIn("dbus-send --session", prepare["run"])
        self.assertIn("dump_desktop_state", prepare["run"])
        for name in ("xvfb", "dbus", "openbox", "picom"):
            self.assertIn(f"$RUNNER_TEMP/{name}.log", prepare["run"])

        verify = next(
            step
            for step in steps
            if step.get("name") == "Verify Linux X11 desktop survived setup steps"
        )
        mock = next(step for step in steps if "deterministic mock" in step.get("name", ""))
        self.assertEqual(verify["if"], "runner.os == 'Linux'")
        self.assertLess(steps.index(verify), steps.index(mock))
        self.assertIn("kill -0", verify["run"])
        self.assertIn("xprop -root _NET_SUPPORTING_WM_CHECK _NET_CLIENT_LIST", verify["run"])
        self.assertIn("^_NET_SUPPORTING_WM_CHECK(WINDOW): window id # 0x", verify["run"])
        self.assertIn("^_NET_CLIENT_LIST(WINDOW):", verify["run"])
        self.assertIn("dbus-send --session", verify["run"])
        self.assertIn("dump_desktop_state", verify["run"])

        stop = next(step for step in steps if step.get("name") == "Stop Linux X11 desktop")
        cleanup = next(
            step
            for step in steps
            if step.get("name") == "Remove plaintext evidence from the runner"
        )
        self.assertEqual(stop["if"], "always() && runner.os == 'Linux'")
        self.assertLess(steps.index(stop), steps.index(cleanup))
        self.assertIn("/proc/$pid/comm", stop["run"])
        self.assertIn('kill "$pid"', stop["run"])

    def test_external_chooser_receives_only_the_key_and_windows_system_root(self):
        # Product capability for direct local execution: the external chooser
        # contract is unchanged, but no Actions workflow step invokes it.
        start = self.orchestrator.index("fn external_choice(")
        end = self.orchestrator.index("fn choose(", start)
        external = self.orchestrator[start:end]
        self.assertIn(".env_clear()", external)
        self.assertIn('.env("TYPESAFE_API_KEY", required("TYPESAFE_API_KEY"))', external)
        self.assertIn('command.env("SYSTEMROOT", system_root)', external)
        self.assertEqual(external.count(".env("), 2)
        self.assertFalse(any("CUA_JEV_CHOOSER_PROGRAM" in str(step) for step in self.candidate_steps()))

    def test_review_driver_is_consumed_from_hash_bound_aggregate(self):
        candidate_text = self.candidate_text()
        self.assertIn("review-measurements.json", candidate_text)
        self.assertIn('measured["review_driver_sha256"]', candidate_text)
        self.assertIn(
            'measured["review_driver_build_profile"] == "debug-review-trust-root"', candidate_text
        )
        self.assertIn("CUA_TEST_DRIVER_BIN", candidate_text)
        self.assertIn("signed-candidate-checksums.txt", candidate_text)
        self.assertIn('measured["code_signing"] == {"status": "not-applicable"', candidate_text)
        self.assertNotIn("cargo build --manifest-path", candidate_text)
        self.assertNotIn("target/release/cua-driver", candidate_text)
        self.assertNotIn("RSA-SHA256", self.evidence_readme)
        self.assertIn("Ed25519", self.evidence_readme)
        self.assertIn("debug review-trust-root", self.evidence_readme)

    def test_no_step_uses_secrets_and_only_measured_binary_runs_demos(self):
        candidate_text = self.candidate_text()
        self.assertNotIn("TYPESAFE_API_KEY", candidate_text)
        self.assertNotIn("${{ secrets.", candidate_text)
        self.assertNotIn("LIVE_TYPESAFE_API_KEY", self.text)
        self.assertFalse(
            any("GH_TOKEN" in str(step.get("env", {})) for step in self.candidate_steps())
        )
        compile_step = next(
            step
            for step in self.candidate_steps()
            if step.get("name", "").startswith("Compile and measure")
        )
        self.assertIn("--no-run --message-format=json", compile_step["run"])
        self.assertIn('name.endswith(f"::{short_name}")', compile_step["run"])
        self.assertIn("assert len(matches) == 1", compile_step["run"])
        self.assertIn("re.fullmatch", compile_step["run"])
        self.assertIn('resolve("authorized_visual_only_window_demo")', compile_step["run"])
        self.assertIn('resolve("authorized_visual_only_primary_desktop_demo")', compile_step["run"])
        self.assertIn("CUA_CANDIDATE_WINDOW_TEST={window_test}", compile_step["run"])
        self.assertIn("CUA_CANDIDATE_DESKTOP_TEST={desktop_test}", compile_step["run"])
        self.assertIn("CUA_CANDIDATE_TEST_BINARY_SHA256", compile_step["run"])
        mock = next(
            step for step in self.candidate_steps() if "deterministic mock" in step.get("name", "")
        )
        self.assertIn("& $env:CUA_CANDIDATE_TEST_BINARY --ignored --exact $env:CUA_CANDIDATE_WINDOW_TEST", mock["run"])
        self.assertIn("& $env:CUA_CANDIDATE_TEST_BINARY --ignored --exact $env:CUA_CANDIDATE_DESKTOP_TEST", mock["run"])
        self.assertLess(
            self.candidate_steps().index(compile_step), self.candidate_steps().index(mock)
        )

    def test_no_external_chooser_install_remains_and_orchestrator_fails_closed(self):
        names = [step.get("name", "") for step in self.candidate_steps()]
        self.assertFalse(any("Jev chooser" in name for name in names))
        self.assertFalse(any("reviewed chooser" in name for name in names))
        self.assertNotIn("CUA_FIXED_CHOOSER", self.text)
        self.assertNotIn("CUA_JEV_LIVE", self.text)
        self.assertNotIn("jev-use-source", self.text)
        self.assertNotIn("Invoke-Expression", self.text)
        self.assertNotIn("bash -c", self.candidate_text())
        # The product orchestrator still supports a reviewed live chooser for
        # direct local execution; Actions never configures it.
        self.assertIn("CUA_JEV_LIVE", self.orchestrator)
        self.assertIn(
            "reviewed chooser program and script paths must be absolute", self.orchestrator
        )
        self.assertIn("set CUA_JEV_MOCK_DEMO=1 or CUA_JEV_LIVE=1", self.orchestrator)

    def test_mock_has_no_chooser_or_key(self):
        mock = next(
            step
            for step in self.candidate_steps()
            if "deterministic mock" in step.get("name", "")
        )
        self.assertEqual(
            mock["env"],
            {
                "CUA_JEV_MOCK_DEMO": "1",
                "CUA_TEST_DRIVER_STDERR": "1",
            },
        )
        self.assertNotIn("CHOOSER", str(mock))
        self.assertNotIn("TYPESAFE", str(mock))
        self.assertIn("--exact $env:CUA_CANDIDATE_WINDOW_TEST", mock["run"])
        self.assertIn("--exact $env:CUA_CANDIDATE_DESKTOP_TEST", mock["run"])

    def test_only_encrypted_schema_validated_evidence_is_uploaded(self):
        uploads = [
            step for step in self.candidate_steps() if "upload-artifact" in step.get("uses", "")
        ]
        self.assertEqual(len(uploads), 1)
        upload = uploads[0]
        self.assertEqual(upload["with"]["path"], "${{ runner.temp }}/encrypted-evidence/")
        self.assertIn("mock-jev-visual-", upload["with"]["name"])
        self.assertNotIn("live-jev-visual", str(upload))
        validate_step = next(
            step
            for step in self.candidate_steps()
            if step.get("name", "").startswith("Fully decode")
        )
        validate = validate_step["run"]
        self.assertIn("ffmpeg -v error -xerror", validate)
        self.assertIn("Draft202012Validator(schema).validate(manifest)", validate)
        self.assertIn(
            'assert sorted(path.name for path in destination.iterdir()) == ["manifest.json", "recording.mp4"]',
            validate,
        )
        self.assertIn('"window": (evidence, "window", "get_window_state", "background")', validate)
        self.assertIn(
            '"primary-desktop": (evidence / "primary-desktop", "desktop", "get_desktop_state", "foreground")',
            validate,
        )
        self.assertIn("destination = output / scope", validate)
        self.assertIn(
            'assert sorted(path.name for path in output.iterdir()) == ["primary-desktop", "window"]',
            validate,
        )
        self.assertIn(
            'perception["signed_extension_archive_sha256"] == measured["archive_sha256"]', validate
        )
        self.assertIn('driver = runtime["driver"]', validate)
        self.assertIn('observation = manifest["observation"]', validate)
        self.assertIn('manifest["environment"] ==', validate)
        self.assertIn('perception["models"] == measured["models"]', validate)
        self.assertIn('chooser["model_id"]', validate)
        self.assertIn('chooser["mode"] == "mock"', validate)
        self.assertIn('chooser["provider"] == "fixture"', validate)
        self.assertIn('recording_value["frame_rate"]', validate)
        self.assertIn('recording_value["edit_operations"]', validate)
        encrypt_step = next(
            step
            for step in self.candidate_steps()
            if step.get("name") == "Encrypt the validated evidence bundles"
        )
        self.assertEqual(
            encrypt_step["env"],
            {
                "CUA_PERCEPTION_EVIDENCE_RECIPIENT": (
                    "${{ vars.EVIDENCE_ARCHIVE_RECIPIENT_PUBLIC_KEY }}"
                )
            },
        )
        self.assertEqual(
            sum(
                "vars.EVIDENCE_ARCHIVE_RECIPIENT_PUBLIC_KEY" in str(step)
                for step in self.candidate_steps()
            ),
            1,
        )
        self.assertNotIn("EVIDENCE_ARCHIVE_KEY", self.text)
        self.assertNotIn("private-key", self.text)
        self.assertIn("evidence_envelope.py encrypt", encrypt_step["run"])
        self.assertIn("--recipient-env CUA_PERCEPTION_EVIDENCE_RECIPIENT", encrypt_step["run"])
        self.assertIn("^recipient_public_key_sha256=[0-9a-f]{64}$", encrypt_step["run"])
        self.assertIn('Write-Host "$scope $fingerprint"', encrypt_step["run"])
        self.assertIn("Remove-Item Env:CUA_PERCEPTION_EVIDENCE_RECIPIENT", encrypt_step["run"])
        self.assertIn('== ["primary-desktop.cuae", "window.cuae"]', encrypt_step["run"])
        self.assertNotIn("CUA_PERCEPTION_EVIDENCE_RECIPIENT", validate)
        steps = self.candidate_steps()
        cleanup = next(
            step
            for step in steps
            if step.get("name") == "Remove plaintext evidence from the runner"
        )
        self.assertLess(steps.index(validate_step), steps.index(encrypt_step))
        self.assertLess(steps.index(encrypt_step), steps.index(upload))
        self.assertLess(steps.index(upload), steps.index(cleanup))
        self.assertEqual(cleanup["if"], "always()")
        for directory in (
            "cua-perception-recordings",
            "cua-perception-evidence",
            "perception-evidence",
            "publish-evidence",
        ):
            self.assertIn(f"'{directory}'", cleanup["run"])
        self.assertIn(
            "-not [string]::IsNullOrWhiteSpace($env:CUA_PERCEPTION_EXTENSION_HOME)", cleanup["run"]
        )
        self.assertIn(
            "$expectedExtensionHome = [IO.Path]::GetFullPath((Join-Path $userProfile $extensionHomeName))",
            cleanup["run"],
        )
        self.assertIn("$extensionHome -ne $expectedExtensionHome", cleanup["run"])
        self.assertIn(
            "[IO.Path]::GetDirectoryName($extensionHome) -ne $userProfile", cleanup["run"]
        )
        self.assertIn("try {", cleanup["run"])
        self.assertIn("} finally {", cleanup["run"])
        self.assertLess(cleanup["run"].index("try {"), cleanup["run"].index("} finally {"))
        self.assertLess(
            cleanup["run"].index("} finally {"),
            cleanup["run"].index("$runnerTemp = [IO.Path]::GetFullPath"),
        )
        self.assertIn(
            "$extensionHomeItem.Attributes -band [IO.FileAttributes]::ReparsePoint", cleanup["run"]
        )
        self.assertIn("Remove-Item -LiteralPath $expectedExtensionHome -Force", cleanup["run"])
        self.assertIn(
            "Remove-Item -LiteralPath $expectedExtensionHome -Recurse -Force", cleanup["run"]
        )
        reparse = cleanup["run"].index(
            "$extensionHomeItem.Attributes -band [IO.FileAttributes]::ReparsePoint"
        )
        unlink = cleanup["run"].index("Remove-Item -LiteralPath $expectedExtensionHome -Force")
        recursive = cleanup["run"].index(
            "Remove-Item -LiteralPath $expectedExtensionHome -Recurse -Force"
        )
        self.assertLess(reparse, unlink)
        self.assertLess(unlink, recursive)
        self.assertNotIn("Remove-Item -Path $env:CUA_PERCEPTION_EXTENSION_HOME", cleanup["run"])
        self.assertNotIn("@'", cleanup["run"])
        self.assertNotIn("python - $env:RUNNER_TEMP", cleanup["run"])
        self.assertIn("$runnerTemp = [IO.Path]::GetFullPath($env:RUNNER_TEMP)", cleanup["run"])
        self.assertIn(
            "$runnerTempItem.Attributes -band [IO.FileAttributes]::ReparsePoint", cleanup["run"]
        )
        self.assertIn("[IO.Path]::GetDirectoryName($target) -ne $runnerTemp", cleanup["run"])
        self.assertIn("Remove-Item -LiteralPath $target -Force", cleanup["run"])
        self.assertIn("Remove-Item -LiteralPath $target -Recurse -Force", cleanup["run"])
        self.assertNotIn("publish-evidence", str(upload))
        self.assertNotIn("manifest.json", str(upload))
        self.assertNotIn("recording.mp4", str(upload))
        self.assertNotIn("raw-manifest.json", str(upload))
        self.assertNotIn("timeline.json", str(upload))

    def test_all_actions_are_full_sha_pinned(self):
        uses = re.findall(r"^\s*-?\s*uses:\s*([^\s#]+)", self.text, re.MULTILINE)
        self.assertTrue(uses)
        for action in uses:
            self.assertRegex(action, r"^[^@]+@[0-9a-f]{40}$")


if __name__ == "__main__":
    unittest.main()
