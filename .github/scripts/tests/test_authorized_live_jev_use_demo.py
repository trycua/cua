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
                    "run_live",
                    "source_sha",
                    "jev_source_sha",
                    "signed_candidate_artifact_id",
                    "signed_candidate_run_id",
                },
            )
            self.assertTrue(all(value["required"] for value in trigger["inputs"].values()))
            self.assertEqual(trigger["inputs"]["run_live"]["type"], "boolean")
        source = self.jobs["source"]["steps"][0]["run"]
        self.assertIn('[[ "$RUN_LIVE_ACKNOWLEDGED" == true ]]', source)
        self.assertEqual(
            self.jobs["source"]["steps"][0]["env"]["RUN_LIVE_ACKNOWLEDGED"],
            "${{ inputs.run_live }}",
        )
        self.assertIn("validate_pr_head 3943", source)
        self.assertNotIn("validate_pr_head 3916", source)
        self.assertIn("validate_merged_pr 3916", source)
        self.assertEqual(
            self.jobs["source"]["steps"][0]["env"]["CANONICAL_JEV_MERGE_SHA"],
            "bdaf8c2570e35254f5e50a317781374efe7aa91a",
        )
        self.assertEqual(
            dispatch_inputs["jev_source_sha"]["description"],
            "Canonical merge commit SHA of merged pull request #3916",
        )
        self.assertIn('[[ "$requested" == "$head_sha" ]]', source)
        self.assertIn('"$head_repo" == "$GITHUB_REPOSITORY"', source)
        self.assertIn('[[ "$state" == closed && "$merged" == true', source)
        self.assertIn('"$merge_sha" == "$expected"', source)
        self.assertNotIn("merge-base --is-ancestor", source)
        self.assertNotIn("pull_request_target", self.text)
        self.assertIn('[[ "$run_id" == "$CANDIDATE_RUN_ID" ]]', source)
        self.assertIn('STAGING-cua-perception-review-candidates-$REQUESTED_SHA', source)
        self.assertIn('.github/workflows/review-cua-perception-pr3943.yml', source)
        self.assertIn('.event <<<"$run_json")" == pull_request', source)
        self.assertIn('[[ "$EVENT_ACTION" == labeled', source)
        self.assertIn('"$EVENT_PR_NUMBER" == 3943', source)
        self.assertIn('"$EVENT_HEAD_SHA" == "$REQUESTED_SHA"', source)
        self.assertIn('"$EVENT_LABEL" == cua-perception-live-review', source)
        self.assertIn('"$run_id" == "$GITHUB_RUN_ID"', source)
        self.assertIn('"$GITHUB_EVENT_NAME" == workflow_dispatch', source)
        self.assertIn('.status <<<"$run_json")" == in_progress', source)
        self.assertIn('.status <<<"$run_json")" == completed', source)
        download = next(step for step in self.jobs["live"]["steps"] if "download-artifact" in step.get("uses", ""))
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
        self.assertEqual(self.jobs["live"]["env"]["CUA_E2E_UNRESTRICTED_GUI"], "1")
        self.assertEqual(
            set(self.jobs["live"]["needs"]),
            {"source", "mock-preflight", "windows-preflight", "linux-x11-preflight"},
        )
        windows = "\n".join(step.get("run", "") for step in self.jobs["windows-preflight"]["steps"])
        linux = "\n".join(step.get("run", "") for step in self.jobs["linux-x11-preflight"]["steps"])
        mock = "\n".join(step.get("run", "") for step in self.jobs["mock-preflight"]["steps"])
        self.assertIn("python3-tk", mock)
        self.assertIn("libx11-dev", mock)
        self.assertIn("libwebkit2gtk-4.1-dev", mock)
        self.assertLess(mock.index("python3-tk"), mock.index("test_fixture.py"))
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

    def test_live_paths_use_private_windows_extension_home_and_runner_temp_evidence(self):
        live_env = self.jobs["live"]["env"]
        self.assertNotIn("CUA_PERCEPTION_EXTENSION_HOME", live_env)
        self.assertNotIn("CUA_PERCEPTION_EVIDENCE_DIR", live_env)
        configure = next(
            step
            for step in self.jobs["live"]["steps"]
            if step.get("name") == "Configure temporary evidence paths"
        )
        self.assertIn(
            '$extensionHomeName = "cua-perception-extension-home-$($env:GITHUB_RUN_ID)-$($env:GITHUB_RUN_ATTEMPT)"',
            configure["run"],
        )
        self.assertIn("$userProfile = [IO.Path]::GetFullPath($env:USERPROFILE)", configure["run"])
        self.assertIn("$userProfileItem.Attributes -band [IO.FileAttributes]::ReparsePoint", configure["run"])
        self.assertIn("[IO.Path]::GetDirectoryName($extensionHome) -ne $userProfile", configure["run"])
        self.assertIn("if (Test-Path -LiteralPath $extensionHome) { throw 'Windows extension home must be fresh' }", configure["run"])
        self.assertIn("New-Item -ItemType Directory -Path $extensionHome", configure["run"])
        self.assertIn("$extensionHomeItem.Attributes -band [IO.FileAttributes]::ReparsePoint", configure["run"])
        self.assertNotIn("LOCALAPPDATA", configure["run"])
        self.assertIn("$extensionHome = Join-Path $env:RUNNER_TEMP 'cua-perception-extension-home/live'", configure["run"])
        self.assertIn('"CUA_PERCEPTION_EXTENSION_HOME=$extensionHome" >> $env:GITHUB_ENV', configure["run"])
        self.assertIn('"CUA_DRIVER_RS_HOME=$extensionHome" >> $env:GITHUB_ENV', configure["run"])
        self.assertIn(
            "CUA_PERCEPTION_EVIDENCE_DIR=$(Join-Path $env:RUNNER_TEMP 'cua-perception-evidence/live')",
            configure["run"],
        )
        self.assertNotRegex(
            self.text,
            r"CUA_PERCEPTION_EXTENSION_HOME[^\n]*github\.workspace",
        )
        self.assertNotIn("github.workspace", configure["run"])
        self.assertFalse(any("${{ runner." in value for value in live_env.values()))

    def test_live_linux_desktop_persists_and_is_rechecked_before_demos(self):
        steps = self.jobs["live"]["steps"]
        prepare = next(
            step
            for step in steps
            if step.get("name") == "Prepare Linux X11 desktop"
        )
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
            self.assertIn(f'$RUNNER_TEMP/{name}.log', prepare["run"])

        verify = next(
            step
            for step in steps
            if step.get("name") == "Verify Linux X11 desktop survived setup steps"
        )
        mock = next(
            step
            for step in steps
            if "deterministic mock" in step.get("name", "")
        )
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
        self.assertIn('measured["code_signing"] == {"status": "not-applicable"', live_text)
        self.assertNotIn("cargo build --manifest-path", live_text)
        self.assertNotIn("target/release/cua-driver", live_text)
        self.assertNotIn("RSA-SHA256", self.evidence_readme)
        self.assertIn("Ed25519", self.evidence_readme)
        self.assertIn("debug review-trust-root", self.evidence_readme)

    def test_only_measured_binary_receives_typesafe_secret_in_one_bounded_step(self):
        secret = next(
            step
            for step in self.jobs["live"]["steps"]
            if "secrets.TYPESAFE_API_KEY" in str(step)
        )
        self.assertEqual(
            sum("secrets.TYPESAFE_API_KEY" in str(step) for step in self.jobs["live"]["steps"]),
            1,
        )
        self.assertEqual(secret["timeout-minutes"], 15)
        self.assertEqual(secret["env"], {
            "GH_TOKEN": "${{ github.token }}",
            "LIVE_TYPESAFE_API_KEY": "${{ secrets.TYPESAFE_API_KEY }}",
        })
        self.assertNotIn("cargo ", secret["run"])
        self.assertIn("CUA_LIVE_TEST_BINARY_SHA256", secret["run"])
        self.assertIn("Remove-Item Env:LIVE_TYPESAFE_API_KEY", secret["run"])
        self.assertIn("Remove-Item Env:TYPESAFE_API_KEY", secret["run"])
        self.assertIn("pulls/3943", secret["run"])
        self.assertIn("pulls/3916", secret["run"])
        self.assertIn("$jevReview.state -ne 'closed'", secret["run"])
        self.assertIn("$jevReview.merged -ne $true", secret["run"])
        self.assertIn("$jevReview.head.repo.full_name -ne $env:GITHUB_REPOSITORY", secret["run"])
        self.assertIn("$jevReview.merge_commit_sha -ne 'bdaf8c2570e35254f5e50a317781374efe7aa91a'", secret["run"])
        self.assertNotIn("$jevReview.head.sha", secret["run"])
        self.assertIn("cua-perception-live-review", secret["run"])
        self.assertIn("Remove-Item Env:GH_TOKEN", secret["run"])
        self.assertIn(
            "& $env:CUA_LIVE_TEST_BINARY --ignored --exact $env:CUA_LIVE_WINDOW_TEST",
            secret["run"],
        )
        self.assertIn(
            "& $env:CUA_LIVE_TEST_BINARY --ignored --exact $env:CUA_LIVE_DESKTOP_TEST",
            secret["run"],
        )
        compile_step = next(
            step
            for step in self.jobs["live"]["steps"]
            if step.get("name", "").startswith("Compile and measure")
        )
        self.assertIn("--no-run --message-format=json", compile_step["run"])
        self.assertIn('name.endswith(f"::{short_name}")', compile_step["run"])
        self.assertIn("assert len(matches) == 1", compile_step["run"])
        self.assertIn("re.fullmatch", compile_step["run"])
        self.assertIn('resolve("authorized_visual_only_window_demo")', compile_step["run"])
        self.assertIn('resolve("authorized_visual_only_primary_desktop_demo")', compile_step["run"])
        self.assertIn("CUA_LIVE_WINDOW_TEST={window_test}", compile_step["run"])
        self.assertIn("CUA_LIVE_DESKTOP_TEST={desktop_test}", compile_step["run"])

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
        secret = next(step for step in live["steps"] if "secrets.TYPESAFE_API_KEY" in str(step))
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
                "CUA_TEST_DRIVER_STDERR": "1",
            },
        )
        self.assertNotIn("CHOOSER", str(mock))
        self.assertNotIn("TYPESAFE", str(mock))
        self.assertIn("--exact $env:CUA_LIVE_WINDOW_TEST", mock["run"])
        self.assertIn("--exact $env:CUA_LIVE_DESKTOP_TEST", mock["run"])

    def test_only_encrypted_schema_validated_evidence_is_uploaded(self):
        uploads = [
            step for step in self.jobs["live"]["steps"] if "upload-artifact" in step.get("uses", "")
        ]
        self.assertEqual(len(uploads), 1)
        upload = uploads[0]
        self.assertEqual(upload["with"]["path"], "${{ runner.temp }}/encrypted-evidence/")
        validate_step = next(
            step
            for step in self.jobs["live"]["steps"]
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
        self.assertIn('destination = output / scope', validate)
        self.assertIn(
            'assert sorted(path.name for path in output.iterdir()) == ["primary-desktop", "window"]',
            validate,
        )
        self.assertIn('perception["signed_extension_archive_sha256"] == measured["archive_sha256"]', validate)
        self.assertIn('driver = runtime["driver"]', validate)
        self.assertIn('observation = manifest["observation"]', validate)
        self.assertIn('manifest["environment"] ==', validate)
        self.assertIn('perception["models"] == measured["models"]', validate)
        self.assertIn('chooser["model_id"]', validate)
        self.assertIn('chooser["mode"] == "live"', validate)
        self.assertIn('chooser["provider"] == "typesafe"', validate)
        self.assertIn('recording_value["frame_rate"]', validate)
        self.assertIn('recording_value["edit_operations"]', validate)
        encrypt_step = next(
            step
            for step in self.jobs["live"]["steps"]
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
                for step in self.jobs["live"]["steps"]
            ),
            1,
        )
        self.assertNotIn("EVIDENCE_ARCHIVE_KEY", self.text)
        self.assertNotIn("private-key", self.text)
        self.assertIn("evidence_envelope.py encrypt", encrypt_step["run"])
        self.assertIn(
            "--recipient-env CUA_PERCEPTION_EVIDENCE_RECIPIENT", encrypt_step["run"]
        )
        self.assertIn(
            "^recipient_public_key_sha256=[0-9a-f]{64}$", encrypt_step["run"]
        )
        self.assertIn('Write-Host "$scope $fingerprint"', encrypt_step["run"])
        self.assertIn(
            "Remove-Item Env:CUA_PERCEPTION_EVIDENCE_RECIPIENT", encrypt_step["run"]
        )
        self.assertIn(
            '== ["primary-desktop.cuae", "window.cuae"]', encrypt_step["run"]
        )
        self.assertNotIn("CUA_PERCEPTION_EVIDENCE_RECIPIENT", validate)
        steps = self.jobs["live"]["steps"]
        cleanup = next(
            step
            for step in steps
            if step.get("name") == "Remove plaintext evidence from the runner"
        )
        self.assertLess(steps.index(validate_step), steps.index(encrypt_step))
        self.assertLess(steps.index(encrypt_step), steps.index(upload))
        self.assertLess(steps.index(upload), steps.index(cleanup))
        self.assertEqual(cleanup["if"], "always()")
        for directory in ("cua-perception-evidence", "perception-evidence", "publish-evidence"):
            self.assertIn(f"'{directory}'", cleanup["run"])
        self.assertIn("-not [string]::IsNullOrWhiteSpace($env:CUA_PERCEPTION_EXTENSION_HOME)", cleanup["run"])
        self.assertIn('$expectedExtensionHome = [IO.Path]::GetFullPath((Join-Path $userProfile $extensionHomeName))', cleanup["run"])
        self.assertIn("$extensionHome -ne $expectedExtensionHome", cleanup["run"])
        self.assertIn("[IO.Path]::GetDirectoryName($extensionHome) -ne $userProfile", cleanup["run"])
        self.assertIn("try {", cleanup["run"])
        self.assertIn("} finally {", cleanup["run"])
        self.assertLess(cleanup["run"].index("try {"), cleanup["run"].index("} finally {"))
        self.assertLess(cleanup["run"].index("} finally {"), cleanup["run"].index("$runnerTemp = [IO.Path]::GetFullPath"))
        self.assertIn("$extensionHomeItem.Attributes -band [IO.FileAttributes]::ReparsePoint", cleanup["run"])
        self.assertIn("Remove-Item -LiteralPath $expectedExtensionHome -Force", cleanup["run"])
        self.assertIn("Remove-Item -LiteralPath $expectedExtensionHome -Recurse -Force", cleanup["run"])
        reparse = cleanup["run"].index("$extensionHomeItem.Attributes -band [IO.FileAttributes]::ReparsePoint")
        unlink = cleanup["run"].index("Remove-Item -LiteralPath $expectedExtensionHome -Force")
        recursive = cleanup["run"].index("Remove-Item -LiteralPath $expectedExtensionHome -Recurse -Force")
        self.assertLess(reparse, unlink)
        self.assertLess(unlink, recursive)
        self.assertNotIn("Remove-Item -Path $env:CUA_PERCEPTION_EXTENSION_HOME", cleanup["run"])
        self.assertNotIn("@'", cleanup["run"])
        self.assertNotIn("python - $env:RUNNER_TEMP", cleanup["run"])
        self.assertIn("$runnerTemp = [IO.Path]::GetFullPath($env:RUNNER_TEMP)", cleanup["run"])
        self.assertIn("$runnerTempItem.Attributes -band [IO.FileAttributes]::ReparsePoint", cleanup["run"])
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
