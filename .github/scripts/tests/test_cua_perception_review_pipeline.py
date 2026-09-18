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


def test_review_pipeline_builds_pending_trust_override_and_exact_artifact_contracts() -> None:
    text = workflow_text()
    for value in (
        "CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64",
        "--features review-trust-root",
        'publisher_id = "cua-review-only"',
        'publisher_name = "Cua REVIEW ONLY"',
        'key_id = "review-only-build-override"',
        'signature_algorithm: "ed25519"',
        "cua-perception-input-linux-x86_64",
        "cua-perception-input-windows-x86_64",
        '("windows", "x86_64-pc-windows-msvc")',
        '("linux-x11", "x86_64-unknown-linux-gnu")',
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
