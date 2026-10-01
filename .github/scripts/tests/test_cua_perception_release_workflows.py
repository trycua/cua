"""Contract tests for Perception metadata sync and first-release tag anchoring."""

from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-please.yml"
CI_WORKFLOW = ROOT / ".github/workflows/ci-release-metadata.yml"
VERSION_PATH = "libs/cua-driver/rust/crates/cua-perception/VERSION"
CHANGELOG_PATH = "libs/cua-driver/rust/crates/cua-perception/CHANGELOG.md"


def load_workflow(path: Path) -> tuple[str, dict]:
    source = path.read_text()
    workflow = yaml.safe_load(source)
    assert isinstance(workflow, dict)
    return source, workflow


def step(workflow: dict, name: str) -> dict:
    steps = workflow["jobs"]["release-please"]["steps"]
    return next(item for item in steps if item.get("name") == name)


def test_release_pr_sync_uses_crate_metadata_and_updates_the_lockfile() -> None:
    source, workflow = load_workflow(RELEASE_WORKFLOW)
    sync = step(workflow, "Synchronize generated files on release pull requests")["run"]

    assert VERSION_PATH in sync
    assert "-p cua-perception --precise \"$PERCEPTION_VERSION\"" in sync
    assert "validate_release_versions.py --product perception" in sync
    assert "git add libs/cua-driver/rust/Cargo.lock" in sync
    assert ".github/releases/cua-perception/VERSION" not in source
    assert "jq -r '.[\\\".\\\"]'" not in source


ANCHOR_SCRIPT = ROOT / ".github/scripts/anchor_perception_release_tag.sh"


def test_candidate_tag_is_exact_lightweight_main_anchor_without_a_release() -> None:
    _, workflow = load_workflow(RELEASE_WORKFLOW)
    steps = workflow["jobs"]["release-please"]["steps"]
    tag_step = step(
        workflow, "Anchor a merged Perception version with a lightweight tag"
    )
    assert tag_step["run"] == "bash .github/scripts/anchor_perception_release_tag.sh"
    script = ANCHOR_SCRIPT.read_text()

    tag_index = steps.index(tag_step)
    for release_step in (
        "Open or update automatic release pull requests",
        "Open or update the targeted release pull request",
        "Synchronize generated files on release pull requests",
    ):
        assert tag_index < next(
            index
            for index, item in enumerate(steps)
            if item.get("name") == release_step
        )
    assert tag_step["if"] == (
        "github.event_name == 'push' && github.ref == 'refs/heads/main'"
    )
    assert tag_step["env"]["GH_TOKEN"] == "${{ steps.app-token.outputs.token }}"
    assert tag_step["env"]["VERSION_PATH"] == VERSION_PATH
    assert "validate_release_versions.py --product perception" in script
    assert 'git merge-base --is-ancestor "$BEFORE_SHA" "$GITHUB_SHA"' in script
    assert 'git merge-base --is-ancestor "$TARGET_SHA" origin/main' in script
    assert "Perception version must increase" in script
    assert 'TAG="cua-perception-v$VERSION"' in script
    assert 'tag_type" != "commit"' in script
    assert 'tag_sha" != "$expected_sha"' in script
    assert '-f ref="refs/tags/$TAG"' in script
    assert '-f sha="$TARGET_SHA"' in script
    assert "--method PATCH" not in script
    assert "gh release" not in script
    assert "/releases" not in script


def test_ci_uses_crate_root_metadata_and_central_version_validation() -> None:
    source, workflow = load_workflow(CI_WORKFLOW)
    steps = workflow["jobs"]["validate"]["steps"]
    validation = next(
        item for item in steps if item.get("name") == "Validate checked-in release versions"
    )
    preflight = next(
        item
        for item in steps
        if item.get("name") == "Preflight trusted Release Please attribution"
    )["run"]

    assert validation["run"] == (
        "python3 .github/scripts/validate_release_versions.py --product all"
    )
    assert VERSION_PATH in preflight
    assert f"--changelog {CHANGELOG_PATH}" in preflight
    assert ".github/releases/cua-perception/VERSION" not in source
    assert ".github/releases/cua-perception/*" in source
    assert "jq -r '.[\\\".\\\"]'" not in source
