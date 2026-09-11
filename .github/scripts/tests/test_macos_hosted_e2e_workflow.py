"""Contract tests for the manually dispatched GitHub-hosted macOS E2E lane."""

from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]


def read(relative_path: str) -> str:
    return (REPO_ROOT / relative_path).read_text()


def test_hosted_macos_probe_is_manual_exact_sha_and_least_privilege() -> None:
    workflow = read(".github/workflows/e2e-rust-macos.yml")

    trigger = workflow.split("permissions:", 1)[0]
    assert "workflow_dispatch:" in trigger
    assert "pull_request:" not in trigger
    assert "push:" not in trigger
    assert "source_sha:" in trigger
    assert "permissions:\n  contents: read\n" in workflow
    assert "id-token: write" not in workflow
    assert "secrets." not in workflow
    assert "runs-on: macos-26" in workflow
    assert "ref: ${{ inputs.source_sha }}" in workflow
    assert "^[0-9a-fA-F]{40}$" in workflow
    assert "persist-credentials: false" in workflow
    assert "github.run_id }}-${{ github.run_attempt" in workflow
    assert "CUA_MACOS_HOSTED_PROBE_DIR: ${{ runner.temp" not in workflow
    assert 'echo "CUA_MACOS_HOSTED_PROBE_DIR=${artifact_dir}" >> "${GITHUB_ENV}"' in workflow
    assert "matrix:\n        lane: [shared, native, capture]" in workflow
    assert "fail-fast: false" in workflow
    assert "needs: probe" in workflow
    assert "scripts/ci/macos/run-hosted-rust-e2e.sh" in workflow
    assert "path: artifacts/cua-driver" in workflow
    assert "needs: [probe, matrix]" in workflow
    assert "cua-driver/macos-hosted-certification@v1" in workflow

    for action in ("actions/checkout", "actions/upload-artifact"):
        line = next(line for line in workflow.splitlines() if f"uses: {action}@" in line)
        revision = line.split("@", 1)[1].split()[0]
        assert len(revision) == 40
        assert all(character in "0123456789abcdef" for character in revision)


def test_hosted_macos_probe_fails_closed_before_gui_capture() -> None:
    probe = read("scripts/ci/macos/probe-hosted-runner.sh")

    for requirement in (
        'GITHUB_ACTIONS:-}" == true',
        'RUNNER_ENVIRONMENT:-}" == github-hosted',
        'GITHUB_EVENT_NAME:-}" == workflow_dispatch',
        "current user must be runner",
        "console user must be runner",
        "SSH sessions cannot seed or certify hosted TCC state",
        "csrutil status",
        "System Integrity Protection must be disabled",
        'launchctl print "gui/${CURRENT_UID}"',
        "pgrep -x WindowServer",
    ):
        assert requirement in probe

    assert "trap write_environment EXIT" in probe
    assert "AXIsProcessTrusted" in read("scripts/ci/macos/verify-hosted-window.swift")
    assert "CGPreflightScreenCaptureAccess" in read(
        "scripts/ci/macos/verify-hosted-window.swift"
    )


def test_hosted_macos_probe_proves_textedit_window_content() -> None:
    probe = read("scripts/ci/macos/probe-hosted-runner.sh")
    verifier = read("scripts/ci/macos/verify-hosted-window.swift")

    assert "CUA HOSTED MACOS GUI PROBE" in probe
    assert "open -a TextEdit" in probe
    assert '"marker_recognized"' in verifier
    assert "VNRecognizeTextRequest" in verifier
    assert "CGWindowListCopyWindowInfo" in verifier
    assert "/usr/sbin/screencapture -x -l" in probe
    assert "/usr/sbin/screencapture -x" in probe
    assert "ScreenCaptureKit" not in verifier
    assert '"permission_attribution_scope"' in verifier
    assert '"owner": "TextEdit"' in verifier
    assert 'result.get("window", {}).get("owner") == "TextEdit"' in probe
    assert 'result.get("window", {}).get("name") == "probe.txt"' in probe
    assert "textedit-window.png" in probe
    assert "display.png" in probe


def test_script_ci_runs_when_hosted_macos_contract_changes() -> None:
    workflow = read(".github/workflows/ci-test-scripts.yml")

    assert '      - ".github/workflows/e2e-rust-macos.yml"' in workflow
    assert '      - "scripts/ci/macos/**"' in workflow

    guide = read("scripts/ci/README.md")
    assert "e2e-rust-macos.yml" in guide
    assert "temporary certificate-backed identity" in " ".join(guide.split())
    assert "supplemental" in guide


def test_hosted_macos_runner_is_strict_and_uses_the_canonical_matrix() -> None:
    runner = read("scripts/ci/macos/run-hosted-rust-e2e.sh")

    for requirement in (
        'GITHUB_ACTIONS:-}" == true',
        'GITHUB_EVENT_NAME:-}" == workflow_dispatch',
        'RUNNER_ENVIRONMENT:-}" == github-hosted',
        'ImageOS:-}" == macos26',
        'CURRENT_USER}" == runner',
        'CONSOLE_USER}" == runner',
        'MODEL}" == VirtualMac*',
        'SIP_STATUS}" == "System Integrity Protection status: disabled."',
        '[[ ! -e "${LOCAL_APP}" ]]',
        '[[ ! -e "${KEYCHAIN}" ]]',
    ):
        assert requirement in runner

    assert "security create-keychain" in runner
    assert "ensure_local_signing_identity" in runner
    assert "security add-trusted-cert" in runner
    assert "-p codeSign" in runner
    assert "security remove-trusted-cert" in runner
    assert "security delete-certificate" in runner
    assert "security set-key-partition-list" in runner
    assert "set-keychain-settings -lut 21600" in runner
    assert 'security list-keychains -d user -s' in runner
    assert '"${ORIGINAL_KEYCHAINS[@]}"' in runner
    assert "run_bounded 30 codesign" in runner
    assert "phase.txt" in runner
    assert "--require-stable-signing" in runner
    assert 'grep -Fq "certificate leaf"' in runner
    assert "ScreenCaptureApprovals.plist" in runner
    assert 'SCREEN_CAPTURE_CLIENT="com.trycua.driver.local"' in runner
    assert "kScreenCaptureApprovalLastAlerted" in runner
    assert "kScreenCaptureApprovalLastUsed" in runner
    assert "screen-capture-approval.txt" in runner
    assert "killall -HUP replayd" in runner
    assert "seed-tcc-guest.sh" in runner
    assert "--expected-client com.trycua.driver.local" in runner
    assert "--dangerously-bypass-approvals" in runner
    assert ".direct_capture_status == \"not_checked\"" in runner
    assert '.source.attribution == "driver-daemon"' in runner
    assert 'bash "${SCRIPT_DIR}/run-rust-e2e.sh"' in runner
    assert 'CUA_E2E_MACOS_DAEMON_SOCKET="${DAEMON_SOCKET}"' in runner
    assert '--socket "${DAEMON_SOCKET}"' in runner
    assert 'trap \'exit 130\' INT' in runner
    assert 'trap \'exit 143\' TERM' in runner
    assert 'bash "${SCRIPT_DIR}/probe-hosted-runner.sh"' in runner
    assert "watch_daemon" in runner
    assert "permissions grant" not in runner
    assert "cleanup-targets.txt" in runner
