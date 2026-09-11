"""Contract tests for the manually dispatched GitHub-hosted macOS probe."""

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
    assert "SCContentFilter(display:" in verifier
    assert '"permission_attribution_scope"' in verifier
    assert '"owner": window.owningApplication?.applicationName' in verifier
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
    assert "does not\nestablish `CuaDriverLocal.app` TCC authorization" in guide
