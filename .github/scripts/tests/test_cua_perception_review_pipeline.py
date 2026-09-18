from __future__ import annotations

import json
from pathlib import Path
import re

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/cd-cua-perception-review-supplied-inputs.yml"
LOCK = ROOT / "libs/cua-driver/rust/crates/cua-perception/scripts/artifacts.lock.json"


def workflow_text() -> str:
    return WORKFLOW.read_text(encoding="utf-8")


def workflow_triggers(parsed: dict) -> dict:
    return parsed.get("on", parsed.get(True))


def test_review_pipeline_declares_equal_literal_trigger_inputs() -> None:
    text = workflow_text()
    parsed = yaml.safe_load(text)
    triggers = workflow_triggers(parsed)
    assert triggers["workflow_dispatch"]["inputs"] == triggers["workflow_call"]["inputs"]
    assert "&review_inputs" not in text
    assert "*review_inputs" not in text


def test_review_pipeline_is_valid_pinned_and_nonpublishing() -> None:
    text = workflow_text()
    parsed = yaml.safe_load(text)
    assert isinstance(parsed, dict)
    assert parsed["permissions"] == {
        "actions": "read",
        "contents": "read",
        "pull-requests": "read",
    }
    uses = re.findall(r"^\s*uses:\s*([^\s#]+)", text, flags=re.MULTILINE)
    assert uses
    assert all(re.fullmatch(r"[^@]+@[0-9a-f]{40}", use) for use in uses)
    lowered = text.lower()
    assert "gh release" not in lowered
    assert "contents: write" not in lowered
    assert "perception_ed25519_private_key_base64" not in lowered
    assert "review_only: true" in lowered
    assert "environment:" not in text
    assert "secrets." not in text
    assert all("permissions" not in job for job in parsed["jobs"].values())


def test_review_pipeline_binds_current_pr_head_and_authenticated_supplied_model() -> None:
    text = workflow_text()
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    detector = next(item for item in lock["artifacts"] if item["role"] == "icon-detect")
    assert detector["url"] is None
    assert detector["size"] == 80_933_219
    assert detector["sha256"] == "d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2"
    assert 'digest.hexdigest() != expected["sha256"]' in text
    assert "refs/pull/3943/head" in text
    assert 'pulls/3943' in text
    assert 'release.get("draft") is not True' in text
    assert 'Accept="application/octet-stream"' in text
    assert "browser_download_url" not in text
    assert text.index("differ from the reviewed size or SHA-256") < text.index(
        "Generate an ephemeral review trust root"
    )
    assert 'test "$EVENT_HEAD_SHA" = "$REQUESTED_SHA"' in text
    assert 'test "$GITHUB_SHA" = "$REQUESTED_SHA"' in text
    assert 'test "$EVENT_LABEL" = cua-perception-live-review' in text


def test_review_pipeline_builds_pending_trust_override_and_exact_artifact_contracts() -> None:
    text = workflow_text()
    parsed = yaml.safe_load(text)
    for value in (
        "CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64",
        "--features review-trust-root",
        'publisher_id = "cua-review-only"',
        'publisher_name = "Cua REVIEW ONLY"',
        'key_id = "review-only-build-override"',
        'signature_algorithm: "ed25519"',
        "cua-perception-input-linux-x86_64",
        "cua-perception-input-windows-x86_64",
        "cua-perception-input-macos-aarch64",
        '("windows", "x86_64-pc-windows-msvc")',
        '("linux-x11", "x86_64-unknown-linux-gnu")',
        '("macos", "aarch64-apple-darwin")',
        "STAGING-cua-perception-review-candidates-",
        'review_driver_relative_path: driverName',
        'review_driver_build_profile: "debug-review-trust-root"',
    ):
        assert value in text
    driver_build = re.search(
        r"cargo build --manifest-path libs/cua-driver/rust/Cargo.toml \\\n+\s+-p cua-driver[^\n]+",
        text,
    )
    assert driver_build and "--release" not in driver_build.group(0)
    assert "const payloadBytes = Buffer.from(JSON.stringify(payload));" in text
    assert "crypto.sign(null, payloadBytes, privateKey)" in text
    assert "crypto.verify(null, payloadBytes, key" in text
    assert "fs.unlinkSync(privatePath)" in text
    aggregate = parsed["jobs"]["aggregate"]
    assert aggregate["outputs"] == {
        "signed_candidate_artifact_id": "${{ steps.upload.outputs.artifact-id }}",
        "producer_run_id": "${{ steps.producer.outputs.run_id }}",
    }
    download = next(
        step
        for step in aggregate["steps"]
        if step.get("name") == "Download all signed target layouts"
    )
    assert download["with"] == {
        "pattern": "review-cua-perception-*-${{ inputs.source_sha }}",
        "path": "staged",
        "merge-multiple": True,
    }
    upload = next(step for step in aggregate["steps"] if step.get("id") == "upload")
    assert upload["with"]["name"] == (
        "STAGING-cua-perception-review-candidates-${{ inputs.source_sha }}"
    )
    assert upload["with"]["path"] == "staged/"


def test_macos_review_candidate_uses_ephemeral_certificate_signing() -> None:
    text = workflow_text()
    script = (ROOT / ".github/scripts/macos-review-codesign.sh").read_text()
    assert "runner: macos-15" in text
    assert "bash .github/scripts/macos-review-codesign.sh" in text
    assert 'code_signing: codeSigning' in text
    for contract in (
        "mktemp -d",
        "security create-keychain",
        "extendedKeyUsage=codeSigning",
        'codesign --force --sign "$identity_hash" --keychain "$keychain_path"',
        "codesign --verify --strict",
        "security delete-keychain",
        'trap cleanup EXIT INT TERM',
        'requirement_output="$({ codesign -d -r- "$driver_path"; } 2>&1)"',
        '"identity": "ephemeral-self-signed-review-only"',
    ):
        assert contract in script
    assert "Developer ID" not in script


def test_review_measurements_are_extracted_from_the_sealed_archive() -> None:
    text = workflow_text()
    for member in (
        'member_bytes("metadata/artifact-manifest.json")',
        'member_bytes("extension.json")',
        'member_json("metadata/runtime-contract.json")',
        'member_json("model-manifest.json")',
        'member_json("modelLedger.json")',
        'member_bytes("metadata/executed-verification.json")',
    ):
        assert member in text
    assert 'for role in ("icon-detect", "ocr-detect", "ocr-recognize")' in text
    assert 'digest(sealed_bytes)' in text
    assert 'gates.get("self-test", {}).get("status") != "passed"' in text
    assert '"sealed_artifact_manifest_sha256": digest(artifact_bytes)' in text
    assert '"sealed_extension_manifest_sha256": digest(extension_bytes)' in text
    assert '"review_driver_version": match.group(0)' in text
    assert 'modelLock.artifacts.find' in text  # download identity only; sealed values replace it below
    assert text.index('measurements.update({') < text.rindex('signed-candidate-checksums.txt')
