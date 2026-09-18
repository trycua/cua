"""Static security contracts for the labeled PR #3943 live review chain."""

from pathlib import Path
import re

import yaml


ROOT = Path(__file__).resolve().parents[3]
TRIGGER = ROOT / ".github/workflows/review-cua-perception-pr3943.yml"
CANDIDATE = ROOT / ".github/workflows/cd-cua-perception-review-supplied-inputs.yml"
LIVE = ROOT / ".github/workflows/authorized-live-jev-use-demo.yml"
MACOS = ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml"


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
    assert "github.event.pull_request.number == 3943" in condition
    assert "github.event.pull_request.head.repo.full_name == github.repository" in condition
    assert "github.event.pull_request.state == 'open'" in condition
    assert "github.event.pull_request.draft == true" in condition
    assert "github.event.label.name == 'cua-perception-live-review'" in condition
    assert "synchronize" not in condition
    assert workflow["concurrency"]["cancel-in-progress"] is True
    assert workflow["concurrency"]["group"] == (
        "cua-perception-live-review-${{ github.event.pull_request.number }}"
    )


def test_gate_revalidates_current_same_repository_heads_and_label() -> None:
    text, workflow = load_workflow(TRIGGER)
    gate = workflow["jobs"]["resolve"]["steps"][0]["run"]
    for contract in (
        '[[ "$GITHUB_REPOSITORY" == "trycua/cua" ]]',
        '[[ "$EVENT_PR_NUMBER" == "3943" ]]',
        '[[ "$(jq -r .state <<<"$pr_json")" == "open" ]]',
        '[[ "$(jq -r .draft <<<"$pr_json")" == "true" ]]',
        '[[ "$(jq -r .head.repo.full_name <<<"$pr_json")" == "$GITHUB_REPOSITORY" ]]',
        '[[ "$(jq -r .head.sha <<<"$pr_json")" == "$EVENT_HEAD_SHA" ]]',
        '.labels | any(.name == "cua-perception-live-review")',
        'gh api "repos/$GITHUB_REPOSITORY/pulls/3916"',
        '[[ "$(jq -r .head.repo.full_name <<<"$jev_json")" == "$GITHUB_REPOSITORY" ]]',
        '[[ "$(jq -r .head.sha <<<"$jev_json")" == "$REVIEWED_JEV_SHA" ]]',
    ):
        assert contract in gate
    assert "secrets." not in text
    assert workflow["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    resolve_env = workflow["jobs"]["resolve"]["steps"][0]["env"]
    assert resolve_env["REVIEWED_JEV_SHA"] == "201732fffd81a40818be7ce2e04269aec962bc42"


def test_review_candidate_executes_release_gates_instead_of_synthesizing_evidence() -> None:
    text, _ = load_workflow(CANDIDATE)
    assert "perception_release.py run-gates" in text
    assert "--evidence executed-verification.json" in text
    assert "assembler:worker-protocol:" not in text


def test_candidate_is_callable_discovers_one_draft_asset_and_returns_artifact_id() -> None:
    text, workflow = load_workflow(CANDIDATE)
    candidate_triggers = triggers(workflow)
    assert set(candidate_triggers) == {"workflow_dispatch", "workflow_call"}
    model_input = candidate_triggers["workflow_call"]["inputs"]["reviewed_model_asset_id"]
    assert model_input["required"] is False
    assert model_input["default"] == ""
    assert "if len(candidates) != 1:" in text
    assert 'release.get("draft") is not True' in text
    assert 'release.get("published_at") is not None' in text
    assert 'digest.hexdigest() != expected["sha256"]' in text
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


def test_chain_passes_exact_outputs_to_existing_protected_live_workflow() -> None:
    trigger_text, workflow = load_workflow(TRIGGER)
    candidate = workflow["jobs"]["candidate"]
    live = workflow["jobs"]["live"]
    assert candidate["uses"] == (
        "./.github/workflows/cd-cua-perception-review-supplied-inputs.yml"
    )
    assert candidate["with"] == {
        "source_sha": "${{ needs.resolve.outputs.source_sha }}",
        "catalog_version": "${{ needs.resolve.outputs.catalog_version }}",
        "expires_unix": "${{ needs.resolve.outputs.expires_unix }}",
    }
    assert "secrets" not in candidate
    assert live["uses"] == "./.github/workflows/authorized-live-jev-use-demo.yml"
    assert live["with"] == {
        "source_sha": "${{ needs.resolve.outputs.source_sha }}",
        "jev_source_sha": "${{ needs.resolve.outputs.jev_source_sha }}",
        "signed_candidate_artifact_id": "${{ needs.candidate.outputs.signed_candidate_artifact_id }}",
        "signed_candidate_run_id": "${{ needs.candidate.outputs.producer_run_id }}",
    }
    assert "secrets" not in live
    assert "secrets: inherit" not in trigger_text
    assert not re.findall(r"^\s*[\w-]+:\s*write\s*$", trigger_text, re.MULTILINE)
    assert not re.findall(r"^\s*environment:", trigger_text, re.MULTILINE)

    live_text, live_workflow = load_workflow(LIVE)
    live_job = live_workflow["jobs"]["live"]
    assert live_job["environment"] == "authorized-live-jev-use-demo"
    secret_steps = [step for step in live_job["steps"] if "${{ secrets." in str(step)]
    assert len(secret_steps) == 1
    assert secret_steps[0]["env"] == {
        "GH_TOKEN": "${{ github.token }}",
        "LIVE_TYPESAFE_API_KEY": "${{ secrets.TYPESAFE_API_KEY }}",
    }
    assert "TYPESAFE_API_KEY" not in "\n".join(
        str(job) for name, job in live_workflow["jobs"].items() if name != "live"
    )
    validation = next(
        step["run"] for step in live_job["steps"]
        if step.get("name", "").startswith("Fully decode")
    )
    assert 'chooser["mode"] == "live"' in validation
    assert 'chooser["provider"] == "typesafe"' in validation


def test_self_hosted_macos_evidence_is_manual_and_not_pr_event_chained() -> None:
    _, workflow = load_workflow(TRIGGER)
    assert "macos" not in workflow["jobs"]
    _, macos_workflow = load_workflow(MACOS)
    assert set(triggers(macos_workflow)) == {"workflow_dispatch"}
    assert macos_workflow["jobs"]["evidence"]["environment"] == "authorized-live-jev-use-demo"
    assert macos_workflow["jobs"]["evidence"]["runs-on"] == [
        "self-hosted",
        "macOS",
        "ARM64",
        "cua-lume-maintainer",
    ]
