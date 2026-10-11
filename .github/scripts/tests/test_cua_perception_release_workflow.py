"""Contract tests for the tag-triggered Cua Perception release workflow."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW_PATH = ROOT / ".github/workflows/cd-cua-perception.yml"
TARGETS = {"x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"}


def workflow() -> dict[str, Any]:
    return yaml.safe_load(WORKFLOW_PATH.read_text(encoding="utf-8"))


def triggers() -> dict[str, Any]:
    parsed = workflow()
    # PyYAML reads the bare `on` key as boolean True.
    return parsed.get("on", parsed.get(True))


def run_text(job: dict[str, Any]) -> str:
    return "\n".join(step.get("run", "") for step in job.get("steps", []))


def needs(job: dict[str, Any]) -> set[str]:
    value = job.get("needs", [])
    return {value} if isinstance(value, str) else set(value)


def test_publication_starts_only_from_the_release_please_tag() -> None:
    on = triggers()
    assert set(on) == {"push", "pull_request"}
    assert on["push"] == {"tags": ["cua-perception-v*"]}
    # No manual dispatch or reusable entry point can reach publication.
    assert "workflow_dispatch" not in on and "workflow_call" not in on
    assert ".github/workflows/cd-cua-perception.yml" in on["pull_request"]["paths"]


def test_release_please_keeps_creating_the_lightweight_anchor_tag() -> None:
    config = json.loads((ROOT / "release-please-config.json").read_text())
    package = config["packages"]["libs/cua-driver/rust/crates/cua-perception"]
    assert package["skip-github-release"] is True
    release_please = (ROOT / ".github/workflows/release-please.yml").read_text()
    anchor = (ROOT / ".github/scripts/anchor_perception_release_tag.sh").read_text()
    assert "run: bash .github/scripts/anchor_perception_release_tag.sh" in release_please
    assert 'TAG="cua-perception-v$VERSION"' in anchor
    # The anchor uses the release app token so the tag push triggers this workflow.
    assert "GH_TOKEN: ${{ steps.app-token.outputs.token }}" in release_please


def test_permissions_are_read_only_except_the_release_job() -> None:
    parsed = workflow()
    assert parsed["permissions"] == {"actions": "read", "contents": "read"}
    for name, job in parsed["jobs"].items():
        permissions = job.get("permissions", {})
        if name == "release":
            assert permissions == {"actions": "read", "contents": "write"}
        elif name == "reviewed-model":
            # Draft release assets are visible only to tokens with push access.
            assert permissions == {"contents": "write"}
            script = run_text(job)
            assert "python" not in script and "cargo" not in script and "bash " not in script
            assert all(
                step.get("with", {}).get("persist-credentials") is False
                for step in job["steps"]
                if "actions/checkout" in step.get("uses", "")
            )
        else:
            assert "write" not in json.dumps(permissions), name


def test_resolve_binds_tag_version_commit_and_main_history() -> None:
    resolve = workflow()["jobs"]["resolve"]
    script = run_text(resolve)
    assert "github.repository == 'trycua/cua'" in resolve["if"]
    assert "head.repo.full_name == github.repository" in resolve["if"]
    assert '[[ "$TAG" == "cua-perception-v$VERSION" ]]' in script
    assert '[[ "$TAG_TYPE" == commit && "$TAG_SHA" == "$SOURCE_SHA" ]]' in script
    assert 'git merge-base --is-ancestor "$SOURCE_SHA" origin/main' in script
    assert "validate_release_versions.py --product perception" in script
    assert 'SOURCE_SHA="$GITHUB_SHA"' in script
    assert "PUBLISH=true" in script and "PUBLISH=false" in script
    assert "KEY_VALID_UNTIL" in script and "+%y%m%d%H%M" in script


def test_reviewed_model_is_verified_against_the_artifact_lock() -> None:
    script = run_text(workflow()["jobs"]["reviewed-model"])
    assert 'select(.role == "icon-detect")' in script
    assert "artifacts.lock.json" in script
    assert '[[ "$ACTUAL_SIZE" == "$SIZE" && "$ACTUAL_SHA256" == "$SHA256" ]]' in script


def test_inputs_cover_every_target_and_publish_only_on_tag_runs() -> None:
    job = workflow()["jobs"]["input"]
    assert {entry["target"] for entry in job["strategy"]["matrix"]["include"]} == TARGETS
    assert needs(job) == {"resolve", "reviewed-model"}
    steps = {step["name"]: step for step in job["steps"] if "name" in step}
    upload = steps["Preserve exact candidate packaging input"]
    assert upload["if"] == "needs.resolve.outputs.publish == 'true'"
    dry_run = steps["Developer-only install check of the unsigned package"]
    assert dry_run["if"] == "needs.resolve.outputs.publish != 'true'"
    assert "--unsigned-archive" in dry_run["run"] and "--catalog" not in dry_run["run"]
    assert "bind-source" in run_text(job) and "perception_release.py validate" in run_text(job)
    assert "--require-hashes" in run_text(job)


def test_candidate_reuses_the_reviewed_signing_workflow_for_the_tag() -> None:
    job = workflow()["jobs"]["candidate"]
    assert job["uses"] == "./.github/workflows/cd-cua-perception-candidate.yml"
    assert job["if"] == "needs.resolve.outputs.publish == 'true'"
    assert needs(job) == {"resolve", "input"}
    inputs = job["with"]
    assert inputs["payload_run_id"] == "${{ fromJSON(github.run_id) }}"
    assert inputs["source_ref"] == "${{ needs.resolve.outputs.source_sha }}"
    assert inputs["reviewed_ref"] == "${{ needs.resolve.outputs.reviewed_ref }}"
    assert set(job["secrets"]) == {"PERCEPTION_ED25519_PRIVATE_KEY_BASE64"}
    assert {entry["target_triple"] for entry in job["strategy"]["matrix"]["include"]} == TARGETS
    # Only the candidate job receives the signing secret.
    for name, other in workflow()["jobs"].items():
        if name != "candidate":
            assert "PERCEPTION_ED25519_PRIVATE_KEY_BASE64" not in json.dumps(other), name
    candidate = yaml.safe_load(
        (ROOT / ".github/workflows/cd-cua-perception-candidate.yml").read_text()
    )
    assert candidate["jobs"]["sign"]["environment"] == "cua-perception-candidate-signing"


def test_verify_installs_each_signed_catalog_with_the_published_driver() -> None:
    job = workflow()["jobs"]["verify"]
    assert needs(job) == {"resolve", "candidate"}
    assert job["if"] == "needs.resolve.outputs.publish == 'true'"
    assert {entry["target"] for entry in job["strategy"]["matrix"]["include"]} == TARGETS
    script = run_text(job)
    assert "libs/cua-driver/scripts/install.sh" in script
    assert "libs/cua-driver/scripts/install.ps1" in script
    assert "perception_release_install_check.py" in script
    assert "--catalog signed/signed-catalog.json" in script
    assert "--unsigned-archive" not in script
    assert "crypto.verify(null, payloadBytes, key" in script
    assert "trust-root.json" in script
    assert "verify-checksums signed/signed-candidate-checksums.txt" in script


def test_release_requires_every_gate_and_the_verified_tag_commit() -> None:
    job = workflow()["jobs"]["release"]
    assert needs(job) == {"resolve", "input", "candidate", "verify"}
    condition = job["if"]
    assert "github.event_name == 'push'" in condition
    assert "startsWith(github.ref, 'refs/tags/cua-perception-v')" in condition
    assert "always()" not in condition and "failure()" not in condition
    script = run_text(job)
    assert '"$HEAD_SHA" != "$GITHUB_SHA"' in script
    assert '"$TAG_SHA" != "$GITHUB_SHA"' in script
    assert '"$SOURCE_SHA" != "$GITHUB_SHA"' in script
    assert "--latest=false" in script
    assert "--verify-tag" in script and "--draft" in script
    assert "--clobber" in script
    # Published releases are immutable: a re-run only accepts identical assets.
    assert "diff -u expected.txt published.txt" in script
    # The archive keeps its signed name so the Driver resolves it beside the catalog.
    assert 'cp "$DIR/$ARCHIVE" "release-upload/$ARCHIVE"' in script
    assert '"release-upload/$BASE.catalog.json"' in script
    assert '.trust == "publisher-verified"' in script
    downloads = [
        step["with"]["pattern"]
        for step in job["steps"]
        if "download-artifact" in step.get("uses", "")
    ]
    assert downloads == [
        "STAGING-cua-perception-candidate-*-${{ needs.resolve.outputs.source_sha }}",
        "cua-perception-install-evidence-*",
    ]


def test_actions_are_pinned_to_commits() -> None:
    for job in workflow()["jobs"].values():
        for step in job.get("steps", []):
            uses = step.get("uses")
            if uses:
                reference = uses.split("@", 1)[1]
                assert len(reference) == 40 and all(c in "0123456789abcdef" for c in reference), uses


def test_component_registry_points_at_the_release_workflow() -> None:
    registry = json.loads((ROOT / ".github/releases/components.json").read_text())
    component = registry["components"]["cua-perception"]
    assert component["builderWorkflow"] == ".github/workflows/cd-cua-perception.yml"
    assert ".github/workflows/cd-cua-perception.yml" in component["changeDetectionPaths"]
