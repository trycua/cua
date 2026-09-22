from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]


def _triggers(path: Path) -> dict:
    workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
    return workflow.get("on", workflow.get(True))


def test_macos_live_evidence_is_manual_exact_sha_and_protected() -> None:
    path = ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml"
    workflow = path.read_text()
    trigger = workflow.split("permissions:", 1)[0]
    triggers = _triggers(path)
    assert set(triggers) == {"workflow_dispatch", "workflow_call"}
    assert triggers["workflow_dispatch"]["inputs"] == triggers["workflow_call"]["inputs"]
    assert "&macos_evidence_inputs" not in trigger and "*macos_evidence_inputs" not in trigger
    assert trigger.count("source_sha:") >= 4
    assert trigger.count("source_pr_number:") == 2
    assert trigger.count("jev_pr_number:") == 2
    assert trigger.count("macos_lume_e2e_run_id:") == 2
    assert trigger.count("signed_arm64_candidate_artifact_id:") == 2
    assert trigger.count("signed_candidate_run_id:") == 2
    assert "pull_request:" not in trigger and "push:" not in trigger
    assert "source_sha:" in trigger and "jev_source_sha:" in trigger
    assert "signed_arm64_candidate_artifact_id:" in trigger
    assert "signed_candidate_run_id:" in trigger
    assert "permissions:\n  actions: read\n  contents: read\n  pull-requests: read\n" in workflow
    assert "runs-on: [self-hosted, macOS, ARM64, cua-lume-maintainer]" in workflow
    assert "environment: authorized-live-jev-use-demo" in workflow
    assert 'validate_pr_head "$REQUESTED_SOURCE_PR"' in workflow
    assert "3943" not in workflow and "3916" not in workflow
    assert "bdaf8c2570e35254f5e50a317781374efe7aa91a" not in workflow
    assert '[[ "$(jq -r .state <<<"$jev_pr_json")" == closed ]]' in workflow
    assert '[[ "$(jq -r .merged <<<"$jev_pr_json")" == true ]]' in workflow
    assert (
        '[[ "$(jq -r .head.repo.full_name <<<"$jev_pr_json")" '
        '== "$GITHUB_REPOSITORY" ]]' in workflow
    )
    assert (
        '[[ "$(jq -r .merge_commit_sha <<<"$jev_pr_json")" == "$REQUESTED_JEV_SHA" ]]' in workflow
    )
    assert "ref: ${{ needs.resolve.outputs.source_sha }}" in workflow
    assert "ref: ${{ needs.resolve.outputs.jev_source_sha }}" in workflow
    assert "persist-credentials: false" in workflow
    assert '[[ "$run_id" == "$CANDIDATE_RUN_ID" ]]' in workflow
    assert "review-cua-perception-macos-$REQUESTED_SHA" in workflow
    assert ".github/workflows/review-cua-perception-candidates.yml" in workflow
    assert ".github/workflows/e2e-rust-macos.yml" in workflow
    assert "cua-driver/macos-lume-certification@v2" in workflow
    assert "cua-driver/macos-lume-direct-result@v1" in workflow
    assert "direct-lume-console" in workflow
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
    assert "CUA_PERCEPTION_EXTENSION_HOME: ${{ github.workspace }}/.cua-perception-home" in workflow
    assert (
        'echo "CUA_PERCEPTION_EVIDENCE_DIR=$RUNNER_TEMP/cua-perception-evidence/live"' in workflow
    )
    assert 'CUA_E2E_UNRESTRICTED_GUI: "1"' in workflow
    assert 'echo "CUA_E2E_RECORDINGS_ROOT=$RUNNER_TEMP/cua-perception-recordings"' in workflow
    assert "CUA_PERCEPTION_EVIDENCE_DIR: ${{ github.workspace }}" not in workflow
    assert "CUA_E2E_RECORDINGS_ROOT: ${{ github.workspace }}" not in workflow
    assert (
        "libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser" not in workflow
    )
    lume_workflow = (ROOT / ".github/workflows/e2e-rust-macos.yml").read_text()
    assert (
        "libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser"
        not in lume_workflow
    )
    assert "environment: authorized-live-jev-use-demo" in lume_workflow
    assert 'measured["target"] == "aarch64-apple-darwin"' in workflow
    assert 'codesign", "--verify", "--strict"' in workflow
    assert '("certificate leaf", "certificate root")' in workflow
    assert "requirement_result.stdout + requirement_result.stderr" in workflow
    assert 'requirement == signing["designated_requirement"]' in workflow
    assert 'signing["certificate_sha256"]' in workflow
    assert 'arches == ["arm64"]' in workflow
    assert 'measured["review_driver_sha256"]' in workflow
    assert "publisher_signature_verified" in workflow
    assert "review-only-publisher-verified" in workflow
    assert workflow.index('CUA_JEV_MOCK_DEMO: "1"') < workflow.index("secrets.TYPESAFE_API_KEY")
    assert "CUA_JEV_LIVE=1" in workflow
    assert "secrets.TYPESAFE_API_KEY" in workflow
    assert "signed-candidate-checksums.txt" in workflow
    assert "crypto.verify(null, payloadBytes, key" in workflow
    assert "catalog.signature" in workflow
    assert "d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4.3.0" in workflow
    assert "Grant the exact review Driver path in this disposable Lume guest" in workflow
    assert "libs/cua-driver/tests/runners/macos-lume/seed-tcc-guest.sh" in workflow
    assert 'CUA_TCC_APP_PATH="$candidate"' in workflow
    assert 'CUA_TCC_EXPECTED_CLIENT="$candidate"' in workflow
    assert (
        workflow.index("Verify signed arm64 Driver and candidate measurements")
        < workflow.index("Grant the exact review Driver path in this disposable Lume guest")
        < workflow.index("Install and measure the signed perception extension")
    )
    assert "Preflight noninteractive Lume privileges and keychains" in workflow
    assert "/usr/bin/sudo -n -v" in workflow
    preflight = workflow.split("- name: Preflight noninteractive Lume privileges and keychains", 1)[
        1
    ].split("- name:", 1)[0]
    assert "secrets.CUA_E2E_SIGNING_KEYCHAIN_PASSWORD" not in workflow
    assert "export CUA_E2E_RUNNER_LIB_ONLY=1" in preflight
    assert "source libs/cua-driver/tests/runners/macos-lume/run-all.sh" in preflight
    assert "unset CUA_E2E_RUNNER_LIB_ONLY" in preflight
    assert "unlock_required_keychains" in preflight
    assert "security unlock-keychain" not in preflight
    assert "CUA_E2E_SIGNING_KEYCHAIN_PASSWORD" not in lume_workflow
    assert "runs-on: [self-hosted, macOS, ARM64, cua-lume-maintainer]" not in lume_workflow
    assert "actions/setup-python@" not in workflow
    assert 'python-version: "3.12"' in workflow
    assert "uv python install 3.12" in workflow
    assert 'python_path="$(uv python find 3.12)"' in workflow
    assert 'echo "$(dirname "$python_path")" >> "$GITHUB_PATH"' in workflow
    assert "driver.chmod(driver.stat().st_mode | 0o111)" in workflow
    assert workflow.index("driver.chmod(driver.stat().st_mode | 0o111)") < workflow.index(
        "seed-tcc-guest.sh"
    )
    assert (
        'daemon_pid_file="$RUNNER_TEMP/cua-review-driver-${GITHUB_RUN_ID}-'
        '${GITHUB_RUN_ATTEMPT}.pid"' in workflow
    )
    assert 'nohup "$candidate" serve --pid-file "$daemon_pid_file" --socket "$socket"' in workflow
    assert 'HOME="$daemon_home"' not in workflow
    assert "Library/Caches/cua-driver/cua-driver.pid" not in workflow
    assert 'echo "$daemon_pid" > "$RUNNER_TEMP/cua-review-driver.pid"' not in workflow
    assert '[[ ! -L "$daemon_pid_file" ]]' in workflow
    assert 'kill -0 "$daemon_pid" 2>/dev/null' in workflow
    assert '"$candidate" call check_permissions \'{"prompt":false}\'' in workflow
    assert '"$candidate" call get_desktop_state "$capture_args" --socket "$socket"' in workflow
    assert "CUA_E2E_MACOS_DAEMON_SOCKET=$socket" in workflow
    assert '--expected-pid "$CUA_REVIEW_DAEMON_PID" stop' in workflow
    assert 'content.startswith(b"\\x89PNG\\r\\n\\x1a\\n")' in workflow
    assert 'permissions.get("accessibility") is True' in workflow
    assert 'permissions.get("screen_recording") is True' in workflow
    assert workflow.index("seed-tcc-guest.sh") < workflow.index('nohup "$candidate" serve')
    start_step = workflow.split(
        "- name: Start the exact review Driver and prove direct desktop capture", 1
    )[1].split("- name:", 1)[0]
    assert start_step.index("trap cleanup_failed_start EXIT") < start_step.index(
        'nohup "$candidate" serve'
    )
    assert 'rm -f -- "$socket" "$daemon_pid_file"' in start_step
    assert 'kill "$daemon_pid" >/dev/null 2>&1 || true' in start_step
    assert 'kill -KILL "$daemon_pid" >/dev/null 2>&1 || true' in start_step
    assert start_step.index('kill -0 "$daemon_pid" 2>/dev/null; then') < start_step.index(
        'rm -f -- "$socket" "$daemon_pid_file"'
    )
    stop_step = workflow.split("- name: Stop the exact review Driver daemon", 1)[1].split(
        "- name:", 1
    )[0]
    assert 'expected_pid_file="$RUNNER_TEMP/cua-review-driver-${GITHUB_RUN_ID}-' in stop_step
    assert '[[ "$CUA_REVIEW_DAEMON_PID_FILE" == "$expected_pid_file" ]]' in stop_step
    assert 'serving_pid="$(<"$expected_pid_file")"' in stop_step
    assert '--expected-pid "$CUA_REVIEW_DAEMON_PID" stop' in stop_step
    assert 'rm -f -- "$expected_socket" "$expected_pid_file"' in stop_step
    assert '[[ ! -e "$expected_pid_file" && ! -L "$expected_pid_file" ]]' in stop_step
    assert "rm -rf" not in start_step and "rm -rf" not in stop_step
    assert workflow.index("Stop the exact review Driver daemon") < workflow.index(
        "Upload only encrypted evidence envelopes"
    )


def test_macos_github_api_check_uses_explicit_http_failure() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    assert "if response.status != 200:" in workflow
    assert 'raise RuntimeError(f"GitHub API returned HTTP {response.status}")' in workflow
    assert "assert response.status == 200" not in workflow


def test_macos_publication_fully_decodes_and_keeps_private_inputs_local() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    assert "for scope in window primary-desktop; do" in workflow
    assert 'ffmpeg -v error -xerror -i "$evidence/recording.mp4"' in workflow
    assert "libs/cua-driver/tests/perception-demo/sanitize_evidence.py" in workflow
    assert '--raw-evidence "$raw"' in workflow
    assert '--output-dir "$publish_root/$scope"' in workflow
    assert '--session-label "$CUA_SESSION_LABEL-$scope"' in workflow
    assert 'python3 -m venv "$validation_venv"' in workflow
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
    assert (
        '"window": (evidence_root / "raw-manifest.json", "window", "get_window_state", "background")'
        in workflow
    )
    assert (
        '"primary-desktop": (evidence_root / "primary-desktop" / "raw-manifest.json", "desktop", "get_desktop_state", "foreground")'
        in workflow
    )
    assert '== ["primary-desktop", "window"]' in workflow
    assert '[[ -f "$raw" && -f "$evidence/timeline.json" ]]' in workflow


def test_macos_encrypts_each_validated_bundle_and_cleans_plaintext_after_upload() -> None:
    path = ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml"
    text = path.read_text()
    workflow = yaml.safe_load(text)
    steps = workflow["jobs"]["evidence"]["steps"]
    validate = next(step for step in steps if step.get("name", "").startswith("Sanitize"))
    encrypt = next(
        step for step in steps if step.get("name") == "Encrypt the validated evidence bundles"
    )
    upload = next(step for step in steps if "upload-artifact" in step.get("uses", ""))
    cleanup = next(
        step for step in steps if step.get("name") == "Remove plaintext evidence from the runner"
    )

    assert encrypt["env"] == {
        "CUA_PERCEPTION_EVIDENCE_RECIPIENT": ("${{ vars.EVIDENCE_ARCHIVE_RECIPIENT_PUBLIC_KEY }}")
    }
    assert sum("vars.EVIDENCE_ARCHIVE_RECIPIENT_PUBLIC_KEY" in str(step) for step in steps) == 1
    assert "EVIDENCE_ARCHIVE_KEY" not in text
    assert "private-key" not in text
    assert "evidence_envelope.py encrypt" in encrypt["run"]
    assert 'envelope_python="$RUNNER_TEMP/perception-validation-venv/bin/python"' in encrypt["run"]
    assert '--input "$plaintext_root/$scope"' in encrypt["run"]
    assert '--output "$encrypted_root/$scope.cuae"' in encrypt["run"]
    assert "--recipient-env CUA_PERCEPTION_EVIDENCE_RECIPIENT" in encrypt["run"]
    assert "^recipient_public_key_sha256=[0-9a-f]{64}$" in encrypt["run"]
    assert 'echo "$scope $fingerprint_line"' in encrypt["run"]
    assert "trap clear_evidence_recipient EXIT" in encrypt["run"]
    assert "unset CUA_PERCEPTION_EVIDENCE_RECIPIENT" in encrypt["run"]
    assert "clear_evidence_key" not in encrypt["run"]
    assert encrypt["run"].rindex("clear_evidence_recipient") < encrypt["run"].rindex("trap - EXIT")
    assert '== ["primary-desktop.cuae", "window.cuae"]' in encrypt["run"]
    assert upload["with"]["path"] == "${{ runner.temp }}/encrypted-macos-evidence/"
    assert "publish-macos-evidence" not in str(upload)
    assert "manifest.json" not in str(upload) and "recording.mp4" not in str(upload)
    assert cleanup["if"] == "always()"
    for directory in (
        "cua-perception-recordings",
        "cua-perception-evidence",
        "perception-evidence",
        "publish-macos-evidence",
        "private-macos-sanitizer-inputs-window",
        "private-macos-sanitizer-inputs-primary-desktop",
    ):
        assert f'"{directory}"' in cleanup["run"]
    assert steps.index(validate) < steps.index(encrypt) < steps.index(upload) < steps.index(cleanup)


def test_live_secret_is_cleared_even_when_the_command_fails() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    live_step = workflow.split(
        "- name: Run bounded live Jev chooser against signed arm64 candidate", 1
    )[1].split("- name:", 1)[0]
    assert "unset LIVE_TYPESAFE_API_KEY" in live_step
    assert "trap clear_typesafe_key EXIT" in live_step
    assert "unset TYPESAFE_API_KEY" in live_step
    assert "pulls/$CUA_E2E_SOURCE_PR_NUMBER" in live_step
    assert "pulls/$CUA_JEV_PR_NUMBER" in live_step
    assert '[[ "$(jq -r .state <<<"$jev_pr_json")" == closed ]]' in live_step
    assert '[[ "$(jq -r .merged <<<"$jev_pr_json")" == true ]]' in live_step
    assert (
        '[[ "$(jq -r .head.repo.full_name <<<"$jev_pr_json")" '
        '== "$GITHUB_REPOSITORY" ]]' in live_step
    )
    assert (
        '[[ "$(jq -r .merge_commit_sha <<<"$jev_pr_json")" == "$CUA_JEV_SOURCE_SHA" ]]' in live_step
    )
    assert '$(jq -r .head.sha <<<"$jev_pr_json")' not in live_step
    assert 'jq -e --arg label "$FIXED_SOURCE_LABEL"' in live_step
    assert "unset GH_TOKEN" in live_step
    command_with_secret = live_step.index('            "$CUA_LIVE_TEST_BINARY"')
    assert live_step.index("export TYPESAFE_API_KEY") < command_with_secret
    assert command_with_secret < live_step.rindex("clear_typesafe_key")
    assert "authorized_visual_only_window_demo" in live_step
    assert "authorized_visual_only_primary_desktop_demo" in live_step


def test_actions_are_commit_pinned() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    for line in workflow.splitlines():
        if "uses:" not in line or "./" in line:
            continue
        revision = line.split("@", 1)[1].split()[0]
        assert len(revision) == 40
        assert all(character in "0123456789abcdef" for character in revision)
