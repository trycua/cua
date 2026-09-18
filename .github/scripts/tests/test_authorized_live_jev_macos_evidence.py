from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]


def test_macos_live_evidence_is_manual_exact_sha_and_protected() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    trigger = workflow.split("permissions:", 1)[0]
    assert "workflow_dispatch:" in trigger
    assert "pull_request:" not in trigger and "push:" not in trigger
    assert "source_sha:" in trigger and "jev_source_sha:" in trigger
    assert "signed_arm64_candidate_artifact_id:" in trigger
    assert "permissions:\n  actions: read\n  contents: read\n  pull-requests: read\n" in workflow
    assert "runs-on: [self-hosted, macOS, ARM64, cua-lume-maintainer]" in workflow
    assert "environment: authorized-live-jev-use-demo" in workflow
    assert "validate_pr_head 3943" in workflow and "validate_pr_head 3916" in workflow
    assert 'ref: ${{ needs.resolve.outputs.source_sha }}' in workflow
    assert 'ref: ${{ needs.resolve.outputs.jev_source_sha }}' in workflow
    assert "persist-credentials: false" in workflow


def test_macos_live_evidence_uses_canonical_lume_and_signed_arm64_candidate() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    assert (
        "CUA_PERCEPTION_EXTENSION_HOME: ${{ github.workspace }}/.cua-perception-home"
        in workflow
    )
    assert (
        "CUA_PERCEPTION_EVIDENCE_DIR: ${{ github.workspace }}/.cua-perception-evidence/live"
        in workflow
    )
    assert "libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser" in workflow
    assert 'measured["target"] == "aarch64-apple-darwin"' in workflow
    assert 'codesign", "--verify", "--strict"' in workflow
    assert '"certificate leaf" in requirement' in workflow
    assert 'arches == ["arm64"]' in workflow
    assert 'measured["review_driver_sha256"]' in workflow
    assert 'publisher_signature_verified' in workflow
    assert 'review-only-publisher-verified' in workflow
    assert workflow.index('CUA_JEV_MOCK_DEMO: "1"') < workflow.index("secrets.TYPESAFE_API_KEY")
    assert 'CUA_JEV_LIVE=1' in workflow
    assert 'secrets.TYPESAFE_API_KEY' in workflow


def test_macos_publication_fully_decodes_and_keeps_private_inputs_local() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    assert 'ffmpeg -v error -xerror -i "$evidence/recording.mp4"' in workflow
    assert "Draft202012Validator(schema).validate(manifest)" in workflow
    assert 'chooser["adapter_source_sha"] == os.environ["CUA_E2E_SOURCE_SHA"]' in workflow
    assert 'chooser["source_sha"] == os.environ["CUA_JEV_SOURCE_SHA"]' in workflow
    assert '"runner_identity_class": "self-hosted-lume"' in workflow
    assert '== ["manifest.json", "recording.mp4"]' in workflow
    assert "path: ${{ runner.temp }}/publish-macos-evidence/" in workflow
    assert "raw-manifest.json" not in workflow
    assert "timeline.json" not in workflow


def test_actions_are_commit_pinned() -> None:
    workflow = (ROOT / ".github/workflows/authorized-live-jev-macos-evidence.yml").read_text()
    for line in workflow.splitlines():
        if "uses:" not in line or "./" in line:
            continue
        revision = line.split("@", 1)[1].split()[0]
        assert len(revision) == 40
        assert all(character in "0123456789abcdef" for character in revision)
