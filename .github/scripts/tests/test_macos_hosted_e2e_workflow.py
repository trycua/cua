"""Contract tests for the GitHub-hosted macOS E2E lane.

Maintainers dispatch it for pull request evidence, and the stable Cua Driver
tag run calls it as an automatic release gate.
"""

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[3]
PROBE = REPO_ROOT / "scripts/ci/macos/probe-hosted-runner.sh"
HOSTED_RUNNER = REPO_ROOT / "scripts/ci/macos/run-hosted-rust-e2e.sh"
SOURCE_SHA = "a" * 40


def read(relative_path: str) -> str:
    return (REPO_ROOT / relative_path).read_text()


def _write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def _hosted_env(tmp_path: Path, **overrides: str) -> dict[str, str]:
    """A fresh environment that satisfies every hosted-runner identity gate."""
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(tmp_path / "home"),
        "RUNNER_TEMP": str(tmp_path / "runner-temp"),
        "GITHUB_ACTIONS": "true",
        "CI": "true",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "RUNNER_ENVIRONMENT": "github-hosted",
        "RUNNER_OS": "macOS",
        "ImageOS": "macos26",
        "CUA_E2E_SOURCE_SHA": SOURCE_SHA,
        "CUA_E2E_INTERNAL_LANE": "shared",
    }
    env.update(overrides)
    return {key: value for key, value in env.items() if value}


PROBE_REFUSALS = [
    ({"GITHUB_ACTIONS": ""}, "GITHUB_ACTIONS must be true"),
    ({"RUNNER_ENVIRONMENT": "self-hosted"}, "runner must be GitHub-hosted"),
    ({"CI": ""}, "CI must be true"),
    ({"GITHUB_EVENT_NAME": "pull_request"}, "probe must be manually dispatched or run by a stable cua-driver-rs tag release gate"),
    (
        {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/main"},
        "probe must be manually dispatched or run by a stable cua-driver-rs tag release gate",
    ),
    (
        {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/tags/lume-v0.3.0"},
        "probe must be manually dispatched or run by a stable cua-driver-rs tag release gate",
    ),
    ({"CUA_E2E_SOURCE_SHA": "main"}, "source SHA must contain 40 hexadecimal characters"),
    (
        {"SSH_CONNECTION": "192.0.2.1 50000 192.0.2.2 22"},
        "SSH sessions cannot seed or certify hosted TCC state",
    ),
]


def _run_probe(tmp_path: Path, env: dict[str, str]) -> tuple[subprocess.CompletedProcess[str], Path]:
    artifact_dir = tmp_path / "probe"
    env = {**env, "CUA_MACOS_HOSTED_PROBE_DIR": str(artifact_dir)}
    completed = subprocess.run(
        ["bash", str(PROBE)], capture_output=True, text=True, env=env, check=False
    )
    return completed, artifact_dir


def _refusal_id(row: tuple[dict[str, str], str]) -> str:
    return ",".join(f"{key}={value or 'unset'}" for key, value in row[0].items())


@pytest.mark.parametrize(
    ("overrides", "message"), PROBE_REFUSALS, ids=[_refusal_id(row) for row in PROBE_REFUSALS]
)
def test_hosted_macos_probe_refuses_outside_a_dispatched_hosted_run(
    tmp_path: Path, overrides: dict[str, str], message: str
) -> None:
    completed, artifact_dir = _run_probe(tmp_path, _hosted_env(tmp_path, **overrides))

    assert completed.returncode == 1
    assert completed.stderr.strip() == message
    environment = json.loads((artifact_dir / "environment.json").read_text())
    assert environment["status"] == "failed"
    assert environment["message"] == message
    # Refusal happens before any system inventory or GUI capture.
    assert sorted(path.name for path in artifact_dir.iterdir()) == ["environment.json"]


@pytest.mark.parametrize(
    "event",
    [
        {"GITHUB_EVENT_NAME": "workflow_dispatch"},
        {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/tags/cua-driver-rs-v0.29.0"},
    ],
    ids=["workflow_dispatch", "stable-tag-push"],
)
def test_hosted_macos_probe_identity_gates_pass_for_a_dispatched_hosted_run(
    tmp_path: Path, event: dict[str, str]
) -> None:
    """Control: the refusal rows fail only because of their one override."""
    completed, _ = _run_probe(tmp_path, _hosted_env(tmp_path, **event))

    assert completed.returncode != 0
    for _, message in PROBE_REFUSALS:
        assert message not in completed.stderr


HOSTED_RUNNER_REFUSALS = [
    ({"GITHUB_ACTIONS": ""}, "requires GitHub Actions"),
    ({"GITHUB_EVENT_NAME": "pull_request"}, "runs only from workflow_dispatch or a stable cua-driver-rs tag release gate"),
    (
        {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/main"},
        "runs only from workflow_dispatch or a stable cua-driver-rs tag release gate",
    ),
    (
        {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/tags/lume-v0.3.0"},
        "runs only from workflow_dispatch or a stable cua-driver-rs tag release gate",
    ),
    ({"RUNNER_ENVIRONMENT": "self-hosted"}, "requires a GitHub-hosted runner"),
    ({"RUNNER_OS": "Linux"}, "requires RUNNER_OS=macOS"),
    ({"ImageOS": "macos15"}, "requires the macos-26 runner image"),
    ({"SSH_TTY": "/dev/ttys001"}, "SSH sessions cannot seed or certify hosted TCC state"),
    (
        {"CUA_E2E_INTERNAL_LANE": "all"},
        "CUA_E2E_INTERNAL_LANE must be shared, native, capture, or browser",
    ),
]


def _run_hosted_runner(
    tmp_path: Path, overrides: dict[str, str]
) -> tuple[subprocess.CompletedProcess[str], Path, Path]:
    """Run a copy whose repository root is tmp_path, with privileged tools recorded."""
    checkout = tmp_path / "checkout"
    script = checkout / "scripts/ci/macos" / HOSTED_RUNNER.name
    script.parent.mkdir(parents=True)
    shutil.copy2(HOSTED_RUNNER, script)
    fake_bin = tmp_path / "bin"
    privileged = tmp_path / "privileged-calls"
    _write_executable(fake_bin / "uname", "echo Darwin")
    for tool in ("security", "sudo", "codesign", "tccutil"):
        _write_executable(fake_bin / tool, f'echo "{tool} $*" >> "{privileged}"')
    env = _hosted_env(tmp_path, **overrides)
    env["PATH"] = f"{fake_bin}:{env['PATH']}"
    completed = subprocess.run(
        ["bash", str(script)], capture_output=True, text=True, env=env, check=False
    )
    return completed, checkout / "artifacts/cua-driver/macos-hosted-bootstrap", privileged


@pytest.mark.skipif(
    os.uname().sysname == "Darwin",
    reason="the refusal path's diagnostics trap reads the real macOS unified log",
)
@pytest.mark.parametrize(
    ("overrides", "message"),
    HOSTED_RUNNER_REFUSALS,
    ids=[_refusal_id(row) for row in HOSTED_RUNNER_REFUSALS],
)
def test_hosted_macos_runner_refuses_before_touching_signing_state(
    tmp_path: Path, overrides: dict[str, str], message: str
) -> None:
    completed, bootstrap, privileged = _run_hosted_runner(tmp_path, overrides)

    assert completed.returncode == 2
    assert completed.stderr.strip() == f"hosted-macos-e2e: {message}"
    assert (bootstrap / "exit-status.txt").read_text() == "2\n"
    assert not privileged.exists(), privileged.read_text()
    assert not (tmp_path / "runner-temp/cua-driver-hosted-signing.keychain-db").exists()


def test_hosted_macos_probe_is_manual_exact_sha_and_least_privilege() -> None:
    workflow = read(".github/workflows/e2e-rust-macos.yml")

    trigger = workflow.split("permissions:", 1)[0]
    assert "workflow_dispatch:" in trigger
    assert "workflow_call:" in trigger
    assert "pull_request:" not in trigger
    assert "push:" not in trigger
    assert "source_sha:" in trigger
    assert "permissions:\n  actions: read\n  contents: read\n  pull-requests: read\n" in workflow
    assert "id-token: write" not in workflow
    assert "secrets." not in workflow
    assert "runs-on: macos-26" in workflow
    assert "ref: ${{ inputs.source_sha }}" in workflow
    assert "CUA_E2E_WORKFLOW_SHA: ${{ github.sha }}" in workflow
    assert "source_sha must match the selected workflow ref tip" in workflow
    assert "^[0-9a-fA-F]{40}$" in workflow
    assert "persist-credentials: false" in workflow
    assert "github.run_id }}-${{ github.run_attempt" in workflow
    assert "CUA_MACOS_HOSTED_PROBE_DIR: ${{ runner.temp" not in workflow
    assert 'echo "CUA_MACOS_HOSTED_PROBE_DIR=${artifact_dir}" >> "${GITHUB_ENV}"' in workflow
    assert "matrix:\n        lane: [shared, native, capture, browser]" in workflow
    assert "fail-fast: false" in workflow
    assert "needs: probe" in workflow
    assert "scripts/ci/macos/run-hosted-rust-e2e.sh" in workflow
    assert "path: artifacts/cua-driver" in workflow
    assert "needs: [probe, matrix]" in workflow
    assert 'lanes: ["shared", "native", "capture", "browser"]' in workflow
    assert "standalone_browser: true" in workflow
    assert "cua-driver/macos-hosted-certification@v1" in workflow
    assert "workflow_ref: $workflow_ref" in workflow
    assert "workflow_sha: $workflow_sha" in workflow
    assert "live_jev_perception" not in workflow
    assert "authorized-live-jev-macos-evidence.yml" not in workflow
    assert "secrets: inherit" not in workflow
    # A called workflow has no mode input; hosted jobs run unless lume is chosen.
    assert workflow.count("    if: inputs.mode != 'lume'\n") == 2
    assert "    if: ${{ always() && inputs.mode != 'lume' }}\n" in workflow
    assert "    if: inputs.mode == 'lume'\n" in workflow
    # Manual dispatches never queue into (and replace) a release-gate call.
    assert (
        "group: e2e-rust-macos-hosted-${{ github.event_name }}-${{ inputs.source_sha }}"
        in workflow
    )

    for action in ("actions/checkout", "actions/upload-artifact"):
        line = next(line for line in workflow.splitlines() if f"uses: {action}@" in line)
        revision = line.split("@", 1)[1].split()[0]
        assert len(revision) == 40
        assert all(character in "0123456789abcdef" for character in revision)


def test_lume_certification_registers_a_direct_console_run_without_a_self_hosted_runner() -> None:
    workflow = read(".github/workflows/e2e-rust-macos.yml")
    runner = read("libs/cua-driver/tests/runners/macos-lume/run-all.sh")

    assert "workflow_dispatch:" in workflow
    assert "source_sha:" in workflow
    assert "direct_lume_run_id:" in workflow
    assert "direct_lume_evidence_sha256:" in workflow
    assert "direct_lume_result_base64:" in workflow
    assert "name: Register direct Lume certification" in workflow
    assert "runs-on: [self-hosted, macOS, ARM64, cua-lume-maintainer]" not in workflow
    assert "runs-on: ubuntu-latest" in workflow
    assert "environment: authorized-live-jev-use-demo" in workflow
    assert "source_sha must match the selected workflow ref tip" in workflow
    assert "cua-driver/macos-lume-direct-result@v1" in workflow
    assert 'schema "cua-driver/macos-lume-certification@v2"' in workflow
    assert 'workflow_path ".github/workflows/e2e-rust-macos.yml"' in workflow
    assert 'kind: "direct-lume-console"' in workflow
    assert "direct-result.json" in workflow
    assert "libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser" not in workflow
    assert "jq -e '.passed == true' certification.json" in workflow
    assert '"${ARTIFACT_DIR}/run-id.txt"' in runner
    assert "cua-driver/macos-lume-direct-result@v1" in runner
    assert '"${ARTIFACT_DIR}/direct-result.json"' in runner
    assert runner.index("BROWSER_STATUS") < runner.index("macos-lume-direct-result@v1")
    assert (
        "name: rust-macos-lume-certification-${{ github.run_id }}-${{ github.run_attempt }}"
        in workflow
    )


def test_hosted_macos_probe_fails_closed_before_gui_capture() -> None:
    probe = read("scripts/ci/macos/probe-hosted-runner.sh")

    for requirement in (
        "current user must be runner",
        "console user must be runner",
        "csrutil status",
        "System Integrity Protection must be disabled",
        'launchctl print "gui/${CURRENT_UID}"',
        "pgrep -x WindowServer",
    ):
        assert requirement in probe

    assert "AXIsProcessTrusted" in read("scripts/ci/macos/verify-hosted-window.swift")
    assert "CGPreflightScreenCaptureAccess" in read("scripts/ci/macos/verify-hosted-window.swift")


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
    assert "run_with_deadline 30 /usr/bin/killall TextEdit" in probe
    assert "pgrep -x TextEdit" in probe


def test_script_ci_runs_when_hosted_macos_contract_changes() -> None:
    workflow = read(".github/workflows/ci-test-scripts.yml")

    assert '      - ".github/workflows/**"' in workflow
    assert '      - "scripts/ci/macos/**"' in workflow

    guide = read("scripts/ci/README.md")
    assert "e2e-rust-macos.yml" in guide
    assert "temporary certificate-backed identity" in " ".join(guide.split())
    assert "supplemental" in guide
    assert "do not install or register a GitHub Actions runner" in guide
    assert "direct result" in guide


def test_hosted_macos_runner_is_strict_and_uses_the_canonical_matrix() -> None:
    runner = read("scripts/ci/macos/run-hosted-rust-e2e.sh")

    for requirement in (
        'CURRENT_USER}" == runner',
        'CONSOLE_USER}" == runner',
        'MODEL}" == VirtualMac*',
        'SIP_STATUS}" == "System Integrity Protection status: disabled."',
        '[[ ! -e "${LOCAL_APP}" ]]',
        '[[ ! -e "${KEYCHAIN}" ]]',
    ):
        assert requirement in runner

    assert "security create-keychain" in runner
    assert "security add-trusted-cert" in runner
    assert runner.index('TRUSTED_IDENTITY="${IDENTITY}"') < runner.index(
        "security add-trusted-cert"
    )
    assert "-p codeSign" in runner
    assert "security remove-trusted-cert" in runner
    assert "security delete-certificate" in runner
    assert "security dump-trust-settings -d" in runner
    assert "security verify-cert" not in runner
    assert "security set-key-partition-list" in runner
    assert "set-keychain-settings -lut 21600" in runner
    assert "security list-keychains -d user -s" in runner
    assert '"${ORIGINAL_KEYCHAINS[@]}"' in runner
    assert "run_bounded 30 codesign" in runner
    assert "--require-stable-signing" in runner
    assert 'grep -Fq "certificate leaf"' in runner
    assert "ScreenCaptureApprovals.plist" in runner
    assert 'SCREEN_CAPTURE_CLIENT="com.trycua.driver.local"' in runner
    assert "kScreenCaptureApprovalLastAlerted" in runner
    assert "kScreenCaptureApprovalLastUsed" in runner
    assert "killall -HUP replayd" in runner
    assert "seed-tcc-guest.sh" in runner
    assert "--expected-client com.trycua.driver.local" in runner
    assert "--dangerously-bypass-approvals" in runner
    assert '.direct_capture_status == "not_checked"' in runner
    assert '.source.attribution == "driver-daemon"' in runner
    assert 'bash "${SCRIPT_DIR}/run-rust-e2e.sh"' in runner
    assert 'CUA_E2E_MACOS_DAEMON_SOCKET="${DAEMON_SOCKET}"' in runner
    assert '--socket "${DAEMON_SOCKET}"' in runner
    assert "trap 'exit 130' INT" in runner
    assert "trap 'exit 143' TERM" in runner
    assert 'bash "${SCRIPT_DIR}/probe-hosted-runner.sh"' in runner
    assert "permissions grant" not in runner


def test_hosted_macos_browser_lane_mirrors_the_lume_standalone_browser_matrix() -> None:
    runner = read("scripts/ci/macos/run-hosted-rust-e2e.sh")
    lume = read("libs/cua-driver/tests/runners/macos-lume/run-all.sh")
    matrix_runner = read("scripts/ci/macos/run-rust-e2e.sh")

    assert "shared|native|capture|browser) ;;" in runner
    # The repo-local matrix runner keeps its own partitions; only the hosted
    # wrapper routes the browser lane to the standalone browser suite.
    assert "shared|native|capture|all) ;;" in matrix_runner
    assert 'STANDALONE_BROWSER_PRODUCTS="chrome,edge"' in runner
    assert '"/Applications/Google Chrome.app|com.google.Chrome|EQHXZ8M8AV"' in runner
    assert '"/Applications/Microsoft Edge.app|com.microsoft.edgemac|UBF8T346G9"' in runner
    assert "--test-requirement \"${browser_requirement}\"" in runner
    platform = read("libs/cua-driver/rust/crates/platform-macos/src/browser/platform.rs")
    for identity in ('"com.google.Chrome"', '"EQHXZ8M8AV"', '"com.microsoft.edgemac"', '"UBF8T346G9"'):
        assert identity in platform
    assert "missing hosted standalone browser" in runner
    assert "hosted standalone browser signature is not valid" in runner
    assert "standalone-browsers.txt" in runner
    # Only Finder detritus is normalized; the vendor requirement still decides.
    assert "-xattrname com.apple.FinderInfo" in runner
    assert "xattr -d com.apple.FinderInfo" in runner
    assert "xattr -c" not in runner
    assert runner.index("xattr -d com.apple.FinderInfo") < runner.index(
        '--test-requirement "${browser_requirement}"'
    )
    assert 'CUA_E2E_BROWSER_PRODUCTS="${STANDALONE_BROWSER_PRODUCTS}"' in runner
    assert 'CUA_TEST_DRIVER_BIN="${CARGO_TARGET_DIR}/release/cua-driver"' in runner
    assert "artifacts/cua-driver/macos-standalone-browser" in runner
    assert "artifacts/cua-driver/macos-standalone-browser" in lume
    assert 'CUA_E2E_ARTIFACT_DIR="${BROWSER_ARTIFACT_DIR}"' in runner
    assert "scripts/ci/run-rust-standalone-browser-e2e.sh" in runner
    assert "scripts/ci/run-rust-standalone-browser-e2e.sh" in lume
    # The browser rows run only after the same bootstrap and daemon checks.
    dispatch = runner.index('bash "${REPO_ROOT}/scripts/ci/run-rust-standalone-browser-e2e.sh"')
    assert runner.index('.source.attribution == "driver-daemon"') < dispatch
    assert runner.index("WATCHDOG_PID=$!") < dispatch
    assert "never shrinks" in runner
