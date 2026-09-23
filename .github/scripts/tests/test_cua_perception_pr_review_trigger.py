"""Static security contracts for the labeled candidate live review chain."""

from pathlib import Path
import re

import yaml


ROOT = Path(__file__).resolve().parents[3]
TRIGGER = ROOT / ".github/workflows/review-cua-perception-candidates.yml"
CANDIDATE = ROOT / ".github/workflows/cd-cua-perception-review-supplied-inputs.yml"
LIVE = ROOT / ".github/workflows/authorized-live-jev-use-demo.yml"
MACOS_ENTRY = ROOT / ".github/workflows/e2e-rust-macos.yml"


def load_workflow(path: Path) -> tuple[str, dict]:
    text = path.read_text(encoding="utf-8")
    workflow = yaml.safe_load(text)
    assert isinstance(workflow, dict)
    return text, workflow


def triggers(workflow: dict) -> dict:
    return workflow.get("on", workflow.get(True))


def test_trigger_is_only_labeled_pull_request() -> None:
    text, workflow = load_workflow(TRIGGER)
    assert triggers(workflow) == {
        "pull_request": {"types": ["labeled"]},
    }
    assert "workflow_dispatch" not in text
    assert "pull_request_target" not in text
    assert "push:" not in text


def test_unlabelled_and_unrelated_pull_requests_skip_before_any_runner() -> None:
    _, workflow = load_workflow(TRIGGER)
    condition = workflow["jobs"]["resolve"]["if"]
    assert "github.event.pull_request.number == 3943" not in condition
    assert "github.event.pull_request.draft == true" not in condition
    assert "github.event.pull_request.head.repo.full_name == github.repository" in condition
    assert "github.event.pull_request.state == 'open'" in condition
    assert "github.event.label.name == 'cua-perception-live-review'" in condition
    assert "synchronize" not in condition
    assert workflow["concurrency"]["cancel-in-progress"] is True
    assert workflow["concurrency"]["group"] == (
        "cua-perception-live-review-${{ github.event.pull_request.number }}"
    )


def test_gate_revalidates_the_current_labeled_head_for_any_pull_request() -> None:
    text, workflow = load_workflow(TRIGGER)
    gate = workflow["jobs"]["resolve"]["steps"][0]["run"]
    for contract in (
        '[[ "$GITHUB_REPOSITORY" == "trycua/cua" ]]',
        '[[ "$EVENT_PR_NUMBER" =~ ^[1-9][0-9]*$ ]]',
        '[[ "$(jq -r .state <<<"$pr_json")" == "open" ]]',
        '[[ "$(jq -r .head.repo.full_name <<<"$pr_json")" == "$GITHUB_REPOSITORY" ]]',
        '[[ "$(jq -r .head.sha <<<"$pr_json")" == "$EVENT_HEAD_SHA" ]]',
        '.labels | any(.name == "cua-perception-live-review")',
        'echo "source_pr_number=$EVENT_PR_NUMBER" >> "$GITHUB_OUTPUT"',
    ):
        assert contract in gate
    assert "3943" not in gate
    assert "3916" not in gate
    assert "jev" not in gate.lower()
    assert '[[ "$(jq -r .draft <<<"$pr_json")" == "true" ]]' not in gate
    assert "secrets." not in text
    assert workflow["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    assert workflow["jobs"]["candidate"]["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    assert workflow["jobs"]["resolve"]["outputs"]["source_pr_number"] == (
        "${{ steps.resolve.outputs.source_pr_number }}"
    )
    assert "jev_source_sha" not in text


def test_review_candidate_executes_release_gates_instead_of_synthesizing_evidence() -> None:
    text, _ = load_workflow(CANDIDATE)
    assert "perception_release.py run-gates" in text
    assert "--evidence executed-verification.json" in text
    assert "assembler:worker-protocol:" not in text


def test_candidate_is_callable_uses_pinned_reviewed_artifact_and_returns_artifact_id() -> None:
    text, workflow = load_workflow(CANDIDATE)
    candidate_triggers = triggers(workflow)
    assert set(candidate_triggers) == {"workflow_dispatch", "workflow_call"}
    assert "reviewed_model_asset_id" not in candidate_triggers["workflow_call"]["inputs"]
    assert 'REVIEWED_ARTIFACT_ID: "10582583541"' in text
    assert 'REVIEWED_RUN_ID: "35438356263"' in text
    assert 'REVIEWED_SOURCE_ASSET_ID: "571471639"' in text
    assert "expected exactly one reviewed model in producer artifact" in text
    assert "CUA_REVIEWED_MODEL_ASSET_ID" in text
    assert "Number(process.env.CUA_REVIEWED_MODEL_ASSET_ID)" in text
    assert "${{ env.CUA_REVIEWED_MODEL_ASSET_ID }}" not in text
    assert "PERCEPTION_ED25519_PRIVATE_KEY_BASE64" not in text
    assert "${{ secrets." not in text
    assert not any("environment" in job for job in workflow["jobs"].values())
    output = candidate_triggers["workflow_call"]["outputs"]["signed_candidate_artifact_id"]
    assert output["value"] == "${{ jobs.aggregate.outputs.signed_candidate_artifact_id }}"
    run_output = candidate_triggers["workflow_call"]["outputs"]["producer_run_id"]
    assert run_output["value"] == "${{ jobs.aggregate.outputs.producer_run_id }}"
    aggregate = workflow["jobs"]["aggregate"]
    assert aggregate["outputs"]["signed_candidate_artifact_id"] == (
        "${{ steps.upload.outputs.artifact-id }}"
    )


def test_pr_review_produces_candidates_without_entering_the_protected_environment() -> None:
    trigger_text, workflow = load_workflow(TRIGGER)
    assert set(workflow["jobs"]) == {"resolve", "candidate"}
    candidate = workflow["jobs"]["candidate"]
    assert candidate["uses"] == ("./.github/workflows/cd-cua-perception-review-supplied-inputs.yml")
    assert candidate["permissions"]["contents"] == "read"
    assert candidate["with"] == {
        "source_pr_number": "${{ needs.resolve.outputs.source_pr_number }}",
        "source_sha": "${{ needs.resolve.outputs.source_sha }}",
        "catalog_version": "${{ needs.resolve.outputs.catalog_version }}",
        "expires_unix": "${{ needs.resolve.outputs.expires_unix }}",
    }
    assert "secrets" not in candidate
    assert "authorized-live-jev-use-demo.yml" not in trigger_text
    assert "secrets: inherit" not in trigger_text
    assert "${{ secrets." not in trigger_text
    assert not re.findall(r"^\s*([\w-]+):\s*write\s*$", trigger_text, re.MULTILINE)
    assert not re.findall(r"^\s*environment:", trigger_text, re.MULTILINE)


def test_protected_candidate_workflow_is_secret_free_and_keeps_environment() -> None:
    live_text, live_workflow = load_workflow(LIVE)
    live_job = live_workflow["jobs"]["candidate"]
    assert live_job["environment"] == "authorized-live-jev-use-demo"
    assert "TYPESAFE_API_KEY" not in live_text
    assert "LIVE_TYPESAFE_API_KEY" not in live_text
    assert "${{ secrets." not in live_text
    assert "run_live" not in live_text
    assert "CUA_JEV_LIVE" not in live_text
    secret_steps = [step for step in live_job["steps"] if "${{ secrets." in str(step)]
    assert secret_steps == []
    assert "TYPESAFE_API_KEY" not in "\n".join(
        str(job) for name, job in live_workflow["jobs"].items() if name != "candidate"
    )
    evidence_step = next(
        step
        for step in live_job["steps"]
        if "CUA_PERCEPTION_EVIDENCE_RECIPIENT" in step.get("env", {})
    )
    assert evidence_step["env"] == {
        "CUA_PERCEPTION_EVIDENCE_RECIPIENT": ("${{ vars.EVIDENCE_ARCHIVE_RECIPIENT_PUBLIC_KEY }}"),
    }
    assert "EVIDENCE_ARCHIVE_KEY" not in live_text
    validation = next(
        step["run"] for step in live_job["steps"] if step.get("name", "").startswith("Fully decode")
    )
    assert 'chooser["mode"] == "mock"' in validation
    assert 'chooser["provider"] == "fixture"' in validation


def test_macos_direct_lume_registration_is_manual_and_not_pr_event_chained() -> None:
    _, workflow = load_workflow(TRIGGER)
    assert "macos" not in workflow["jobs"]
    entry_text, entry_workflow = load_workflow(MACOS_ENTRY)
    assert set(triggers(entry_workflow)) == {"workflow_dispatch"}
    assert "pull_request:" not in entry_text and "pull_request_target" not in entry_text
    assert "live-jev-perception" not in entry_workflow["jobs"]
    assert "authorized-live-jev-macos-evidence.yml" not in entry_text
