from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW_PATH = ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml"


def _workflow() -> tuple[str, dict]:
    text = WORKFLOW_PATH.read_text(encoding="utf-8")
    parsed = yaml.safe_load(text)
    return text, parsed


def test_macos_workflow_only_attests_direct_lume_on_github_hosted_linux() -> None:
    text, workflow = _workflow()
    triggers = workflow.get("on", workflow.get(True))
    assert set(triggers) == {"workflow_dispatch", "workflow_call"}
    assert triggers["workflow_dispatch"]["inputs"] == triggers["workflow_call"]["inputs"]
    assert set(workflow["jobs"]) == {"attest"}
    assert workflow["jobs"]["attest"]["runs-on"] == "ubuntu-latest"
    assert "self-hosted" not in text
    assert "cua-lume-maintainer" not in text
    assert "secrets.TYPESAFE_API_KEY" not in text
    assert "CUA_JEV_LIVE" not in text
    assert "cargo test" not in text
    assert "seed-tcc-guest.sh" not in text
    assert "run-all.sh" not in text


def test_macos_attestation_is_manual_exact_sha_and_protected() -> None:
    text, workflow = _workflow()
    inputs = workflow.get("on", workflow.get(True))["workflow_dispatch"]["inputs"]
    assert set(inputs) == {
        "source_pr_number",
        "source_sha",
        "jev_pr_number",
        "jev_source_sha",
        "signed_arm64_candidate_artifact_id",
        "signed_candidate_run_id",
        "macos_lume_e2e_run_id",
    }
    assert workflow["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    assert workflow["jobs"]["attest"]["environment"] == "authorized-live-jev-use-demo"
    assert '[[ "$GITHUB_EVENT_NAME" == workflow_dispatch ]]' in text
    assert "cua-perception-live-review" in text
    assert ".github/workflows/review-cua-perception-candidates.yml" in text
    assert ".github/workflows/e2e-rust-macos.yml" in text
    assert "review-cua-perception-macos-$REQUESTED_SHA" in text


def test_macos_attestation_requires_successful_direct_console_result() -> None:
    text, _ = _workflow()
    for value in (
        "cua-driver/macos-lume-certification@v2",
        "cua-driver/macos-lume-direct-result@v1",
        "direct-lume-console",
        ".execution.result.standalone_browser == true",
        ".execution.result.passed == true",
        ".passed == true",
        "cua-driver/macos-direct-lume-attestation@v1",
        "macos-direct-lume-attestation-${{ inputs.source_sha }}",
    ):
        assert value in text
    assert ".execution.evidence_sha256" in text
    assert "^[0-9a-f]{64}$" in text


def test_actions_are_commit_pinned() -> None:
    text, _ = _workflow()
    for line in text.splitlines():
        if "uses:" not in line or "./" in line:
            continue
        revision = line.split("@", 1)[1].split()[0]
        assert len(revision) == 40
        assert all(character in "0123456789abcdef" for character in revision)
