from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]


def test_macos_live_evidence_is_manual_exact_sha_and_protected() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    trigger = workflow.split("permissions:", 1)[0]
    assert "workflow_dispatch:" in trigger and "workflow_call:" not in trigger
    assert "&macos_evidence_inputs" not in trigger and "*macos_evidence_inputs" not in trigger
    assert trigger.count("source_sha:") == 2
    assert trigger.count("signed_arm64_candidate_artifact_id:") == 1
    assert trigger.count("signed_candidate_run_id:") == 1
    assert "pull_request:" not in trigger and "push:" not in trigger
    assert "source_sha:" in trigger and "jev_source_sha:" in trigger
    assert "signed_arm64_candidate_artifact_id:" in trigger
    assert "signed_candidate_run_id:" in trigger
    assert "permissions:\n  actions: read\n  contents: read\n  pull-requests: read\n" in workflow
    assert "runs-on: [self-hosted, macOS, ARM64, cua-lume-maintainer]" in workflow
    assert "environment: authorized-live-jev-use-demo" in workflow
    assert "validate_pr_head 3943" in workflow and "validate_pr_head 3916" in workflow
    assert 'ref: ${{ needs.resolve.outputs.source_sha }}' in workflow
    assert 'ref: ${{ needs.resolve.outputs.jev_source_sha }}' in workflow
    assert "persist-credentials: false" in workflow
    assert '[[ "$run_id" == "$CANDIDATE_RUN_ID" ]]' in workflow
    assert 'STAGING-cua-perception-review-candidates-$REQUESTED_SHA' in workflow
    assert '.github/workflows/review-cua-perception-pr3943.yml' in workflow
    assert '[[ "$(jq -r .event <<<"$run_json")" == pull_request ]]' in workflow
    assert "run-id: ${{ needs.resolve.outputs.candidate_run_id }}" in workflow


def test_macos_dispatch_accepts_only_a_completed_successful_exact_producer_run() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    assert 'if [[ "$GITHUB_EVENT_NAME" == pull_request ]]' not in workflow
    assert "EVENT_HEAD_REPOSITORY" not in workflow
    assert "EVENT_LABEL" not in workflow
    assert '[[ "$GITHUB_EVENT_NAME" == workflow_dispatch ]]' in workflow
    assert '[[ "$(jq -r .status <<<"$run_json")" == completed ]]' in workflow
    assert '[[ "$(jq -r .conclusion <<<"$run_json")" == success ]]' in workflow


def test_macos_live_evidence_uses_canonical_lume_and_signed_arm64_candidate() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    assert (
        "CUA_PERCEPTION_EXTENSION_HOME: ${{ github.workspace }}/.cua-perception-home"
        in workflow
    )
    assert (
        "CUA_PERCEPTION_EVIDENCE_DIR: ${{ github.workspace }}/.cua-perception-evidence/live"
        in workflow
    )
    assert 'CUA_E2E_UNRESTRICTED_GUI: "1"' in workflow
    assert "CUA_E2E_RECORDINGS_ROOT: ${{ github.workspace }}/.cua-perception-recordings" in workflow
    assert "libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser" in workflow
    assert 'measured["target"] == "aarch64-apple-darwin"' in workflow
    assert 'codesign", "--verify", "--strict"' in workflow
    assert '("certificate leaf", "certificate root")' in workflow
    assert "requirement_result.stdout + requirement_result.stderr" in workflow
    assert 'requirement == signing["designated_requirement"]' in workflow
    assert 'signing["certificate_sha256"]' in workflow
    assert 'arches == ["arm64"]' in workflow
    assert 'measured["review_driver_sha256"]' in workflow
    assert 'publisher_signature_verified' in workflow
    assert 'review-only-publisher-verified' in workflow
    assert workflow.index('CUA_JEV_MOCK_DEMO: "1"') < workflow.index("secrets.TYPESAFE_API_KEY")
    assert 'CUA_JEV_LIVE=1' in workflow
    assert 'secrets.TYPESAFE_API_KEY' in workflow
    assert "signed-candidate-checksums.txt" in workflow
    assert "crypto.verify(null, payloadBytes, key" in workflow
    assert "catalog.signature" in workflow
    assert "d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4.3.0" in workflow
    assert "Grant the exact review Driver path in this disposable Lume guest" in workflow
    assert "libs/cua-driver/tests/runners/macos-lume/seed-tcc-guest.sh" in workflow
    assert 'CUA_TCC_APP_PATH="$candidate"' in workflow
    assert 'CUA_TCC_EXPECTED_CLIENT="$candidate"' in workflow
    assert workflow.index("Verify signed arm64 Driver and candidate measurements") < workflow.index(
        "Grant the exact review Driver path in this disposable Lume guest"
    ) < workflow.index("Install and measure the signed perception extension")
    assert "Preflight noninteractive Lume privileges, keychains, and browsers" in workflow
    assert "/usr/bin/sudo -n -v" in workflow
    assert "/usr/bin/security unlock-keychain" in workflow
    assert "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" in workflow
    assert "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" in workflow
    assert "driver.chmod(driver.stat().st_mode | 0o111)" in workflow
    assert workflow.index("driver.chmod(driver.stat().st_mode | 0o111)") < workflow.index(
        "seed-tcc-guest.sh"
    )
    assert 'nohup "$candidate" serve --socket "$socket"' in workflow
    assert '"$candidate" call check_permissions \'{"prompt":false}\'' in workflow
    assert '"$candidate" call get_desktop_state "$capture_args" --socket "$socket"' in workflow
    assert 'CUA_E2E_MACOS_DAEMON_SOCKET=$socket' in workflow
    assert '--expected-pid "$CUA_REVIEW_DAEMON_PID" stop' in workflow
    assert 'content.startswith(b"\\x89PNG\\r\\n\\x1a\\n")' in workflow
    assert 'permissions.get("accessibility") is True' in workflow
    assert 'permissions.get("screen_recording") is True' in workflow
    assert workflow.index("seed-tcc-guest.sh") < workflow.index('nohup "$candidate" serve')
    start_step = workflow.split(
        "- name: Start the exact review Driver and prove direct desktop capture", 1
    )[1].split("- name:", 1)[0]
    assert start_step.index("trap cleanup_failed_start EXIT") < start_step.index(
        'echo "$daemon_pid" >'
    )
    assert workflow.index("Stop the exact review Driver daemon") < workflow.index(
        "Upload only decoded video"
    )


def test_macos_publication_fully_decodes_and_keeps_private_inputs_local() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    assert 'ffmpeg -v error -xerror -i "$evidence/recording.mp4"' in workflow
    assert "libs/cua-driver/tests/perception-demo/sanitize_evidence.py" in workflow
    assert "--raw-evidence \"$raw\"" in workflow
    assert "--output-dir \"$RUNNER_TEMP/publish-macos-evidence\"" in workflow
    assert "python3 -m venv \"$validation_venv\"" in workflow
    assert 'uv pip install --no-config --python "$validation_venv/bin/python"' in workflow
    assert "uv pip install --no-config --system" not in workflow
    assert "Draft202012Validator(schema).validate(manifest)" in workflow
    assert 'manifest["fixture"] == {"id": "visual-only-canvas/v1"' in workflow
    assert 'result["status"] == "passed" and result["stale_capture_refused"] is True' in workflow
    assert 'perception["self_test"] == measured["self_test"]' in workflow
    assert 'perception["models"] == measured["models"]' in workflow
    assert 'perception["onnx_runtime"] == measured["onnx_runtime"]' in workflow
    assert 'measured["code_signing"]["identity"] == "ephemeral-self-signed-review-only"' in workflow
    assert 'subprocess.run(["ffprobe"' in workflow
    assert 'chooser["adapter_source_sha"] == os.environ["CUA_E2E_SOURCE_SHA"]' in workflow
    assert 'chooser["source_sha"] == os.environ["CUA_JEV_SOURCE_SHA"]' in workflow
    assert '"runner_identity_class": "self-hosted-lume"' in workflow
    assert '== ["manifest.json", "recording.mp4"]' in workflow
    assert "path: ${{ runner.temp }}/publish-macos-evidence/" in workflow
    assert '[[ -f "$raw" && -f "$evidence/timeline.json" ]]' in workflow
    assert "path: ${{ runner.temp }}/publish-macos-evidence/" in workflow


def test_live_secret_is_cleared_even_when_the_command_fails() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    live_step = workflow.split(
        "- name: Run bounded live Jev chooser against signed arm64 candidate", 1
    )[1].split("- name:", 1)[0]
    assert "unset LIVE_TYPESAFE_API_KEY" in live_step
    assert "trap clear_typesafe_key EXIT" in live_step
    assert "unset TYPESAFE_API_KEY" in live_step
    assert 'pulls/3943' in live_step and 'pulls/3916' in live_step
    assert 'jq -e \'.labels | any(.name == "cua-perception-live-review")\'' in live_step
    assert "unset GH_TOKEN" in live_step
    command_with_secret = live_step.index('            "$CUA_LIVE_TEST_BINARY"')
    assert live_step.index("export TYPESAFE_API_KEY") < command_with_secret
    assert command_with_secret < live_step.rindex("clear_typesafe_key")


def test_actions_are_commit_pinned() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    for line in workflow.splitlines():
        if "uses:" not in line or "./" in line:
            continue
        revision = line.split("@", 1)[1].split()[0]
        assert len(revision) == 40
        assert all(character in "0123456789abcdef" for character in revision)
