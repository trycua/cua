from __future__ import annotations

import json
from pathlib import Path
import subprocess

import pytest

import release_attribution
import validate_release_please_tags


ROOT = Path(__file__).resolve().parents[3]
PATH = "libs/cua-driver/rust/crates/cua-perception"
INFERENCE_PATH = "libs/cua-driver/experiments/cua-perception-inference"
TAG_PREFIX = "cua-perception-v"
COMPANION_PATHS = (
    "libs/cua-driver/rust/Cargo.toml",
    "libs/cua-driver/rust/Cargo.lock",
)


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, text=True, capture_output=True
    ).stdout.strip()


def test_perception_is_excluded_from_driver_without_becoming_a_component():
    config = json.loads((ROOT / "release-please-config.json").read_text())
    manifest = json.loads((ROOT / ".release-please-manifest.json").read_text())
    registry = json.loads((ROOT / ".github/releases/components.json").read_text())

    assert PATH in config["packages"]["libs/cua-driver"]["exclude-paths"]
    assert INFERENCE_PATH in config["packages"]["libs/cua-driver"]["exclude-paths"]
    assert set(COMPANION_PATHS).isdisjoint(config["packages"]["libs/cua-driver"]["exclude-paths"])
    assert PATH not in config["packages"]
    assert PATH not in manifest
    assert "cua-perception" not in registry["components"]
    exclusions = registry["components"]["cua-driver-rs"]["changeDetectionExcludePaths"]
    assert PATH in exclusions
    assert INFERENCE_PATH in exclusions
    companions = registry["components"]["cua-driver-rs"]["changeDetectionCompanionPaths"]
    assert set(companions) == set(COMPANION_PATHS)


@pytest.mark.parametrize(
    ("title", "labels", "pull_body", "accepted", "error"),
    [
        ("test(cua-perception): add protocol fixtures", [], "", False, "no-release"),
        ("test(cua-perception): add protocol fixtures", ["no-release"], "", True, None),
        ("build: measure inference feasibility", ["no-release"], "", True, None),
        ("docs(cua-perception): document protocol", ["no-release"], "", True, None),
        ("feat: add offline visual parsing", [], "", False, "cannot use"),
        ("fix(other): correct framing", ["no-release"], "", False, "cannot use"),
        ("perf(cua-perception/protocol): reduce copies", ["no-release"], "", False, "cannot use"),
        ("revert(cua-perception,driver): restore framing", [], "", False, "cannot use"),
        ("feat!: publish protocol", ["no-release"], "", False, "cannot use"),
        ("build(cua-perception)!: change protocol", ["no-release"], "", False, "cannot use"),
        (
            "test(cua-perception): add fixtures",
            ["no-release"],
            "BEGIN_COMMIT_OVERRIDE\nfeat: publish worker\nEND_COMMIT_OVERRIDE",
            False,
            "commit override",
        ),
        (
            "docs(cua-perception): document protocol",
            ["no-release"],
            "BEGIN_COMMIT_OVERRIDE\nrevert: restore release behavior\nEND_COMMIT_OVERRIDE",
            False,
            "commit override",
        ),
    ],
)
def test_release_metadata_title_and_label_fixtures(title, labels, pull_body, accepted, error):
    allow_non_release = "no-release" in labels
    if accepted:
        release_attribution.validate_pr_title(
            title,
            require_release=True,
            allow_non_release=allow_non_release,
            forbid_any_releasing_title=True,
            pull_body=pull_body,
        )
    else:
        with pytest.raises(release_attribution.ReleaseError, match=error):
            release_attribution.validate_pr_title(
                title,
                require_release=True,
                allow_non_release=allow_non_release,
                forbid_any_releasing_title=True,
                pull_body=pull_body,
            )


@pytest.mark.parametrize(
    ("paths", "perception_only"),
    [
        ([f"{PATH}/src/main.rs", *COMPANION_PATHS], True),
        ([PATH], True),
        ([INFERENCE_PATH], True),
        (
            [
                f"{INFERENCE_PATH}/src/main.rs",
                "libs/cua-driver/docs/cua-perception-rust-inference-spike.md",
            ],
            True,
        ),
        (["libs/cua-driver/docs/cua-perception-rust-inference-spike.md", *COMPANION_PATHS], True),
        ([f"{PATH}/src/main.rs", "libs/cua-driver/rust/crates/cua-driver/src/main.rs"], False),
        ([f"{PATH}/src/main.rs", "README.md"], False),
        (["libs/cua-driver/docs/cua-perception-rust-inference-spike.md.bak"], False),
        ([*COMPANION_PATHS], False),
        ([], False),
    ],
)
def test_perception_diff_classification(paths, perception_only):
    assert release_attribution.is_perception_only_diff(paths) is perception_only


def test_workflows_apply_scope_and_attribution_exclusions():
    metadata = (ROOT / ".github/workflows/ci-release-metadata.yml").read_text()
    driver = (ROOT / ".github/workflows/cd-rust-cua-driver.yml").read_text()

    assert f"{PATH}/*)" in metadata
    assert 'index("no-release") != null' in metadata
    assert "classify-perception-diff" in metadata
    assert "perception_only=$PERCEPTION_ONLY" in metadata
    assert '--forbid-any-releasing-title --pull-body "$PR_BODY"' in metadata
    for excluded_path in (PATH, INFERENCE_PATH):
        assert metadata.count(f"--exclude-path {excluded_path}") == 1
        assert driver.count(f"--exclude-path {excluded_path}") == 2
    for companion_path in COMPANION_PATHS:
        assert metadata.count(f"--exclude-companion-path {companion_path}") == 1
        assert driver.count(f"--exclude-companion-path {companion_path}") == 2


def test_release_metadata_cli_enforces_unregistered_scope(capsys):
    common = [
        "validate-title",
        "--require-release",
        "--forbid-any-releasing-title",
    ]
    assert (
        release_attribution.main(
            [*common, "--title", "feat(cua-perception): add worker", "--allow-non-release"]
        )
        == 1
    )
    assert "Perception-only changes cannot use" in capsys.readouterr().err

    assert (
        release_attribution.main(
            [
                *common,
                "--title",
                "test(cua-perception): add protocol fixtures",
                "--allow-non-release",
            ]
        )
        == 0
    )
    assert "release title is valid" in capsys.readouterr().out


def test_mixed_driver_diff_preserves_normal_release_title_rules():
    paths = [f"{PATH}/src/main.rs", "libs/cua-driver/rust/crates/cua-driver/src/main.rs"]
    assert release_attribution.is_perception_only_diff(paths) is False
    release_attribution.validate_pr_title(
        "feat: add integrated visual parsing", require_release=True
    )


def test_driver_release_range_excludes_perception_commits(tmp_path: Path):
    git(tmp_path, "init", "-b", "main")
    git(tmp_path, "config", "user.name", "Release Test")
    git(tmp_path, "config", "user.email", "release-test@example.com")
    driver_file = tmp_path / "libs/cua-driver/rust/crates/cua-driver/src/main.rs"
    driver_file.parent.mkdir(parents=True)
    driver_file.write_text("driver\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "fix(cua-driver): seed driver")
    git(tmp_path, "tag", "cua-driver-rs-v1.0.0")

    perception_file = tmp_path / PATH / "src/main.rs"
    perception_file.parent.mkdir(parents=True)
    perception_file.write_text("protocol fixture\n")
    for companion_path in COMPANION_PATHS:
        companion = tmp_path / companion_path
        companion.parent.mkdir(parents=True, exist_ok=True)
        companion.write_text(f"updated for {PATH}\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "test(cua-perception): add protocol fixtures")

    inference_file = tmp_path / INFERENCE_PATH / "src/main.rs"
    inference_file.parent.mkdir(parents=True)
    inference_file.write_text("inference spike\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "build(cua-perception): measure inference feasibility")

    commits = release_attribution.commits_in_range(
        tmp_path,
        "cua-driver-rs-v1.0.0",
        "HEAD",
        ["libs/cua-driver"],
        [PATH, INFERENCE_PATH],
        COMPANION_PATHS,
    )
    assert commits == []

    driver_file.write_text("driver change\n")
    perception_file.write_text("protocol fixture with driver integration\n")
    (tmp_path / COMPANION_PATHS[0]).write_text("workspace with driver integration\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "fix(cua-driver): integrate perception input")
    commits = release_attribution.commits_in_range(
        tmp_path,
        "cua-driver-rs-v1.0.0",
        "HEAD",
        ["libs/cua-driver"],
        [PATH, INFERENCE_PATH],
        COMPANION_PATHS,
    )
    assert [commit.subject for commit in commits] == ["fix(cua-driver): integrate perception input"]


def test_release_tag_validator_has_no_perception_tag_contract(tmp_path: Path):
    git(tmp_path, "init", "-b", "main")
    git(tmp_path, "config", "user.name", "Release Test")
    git(tmp_path, "config", "user.email", "release-test@example.com")
    (tmp_path / "README.md").write_text("fixture\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "chore: seed fixture")

    config = json.loads((ROOT / "release-please-config.json").read_text())
    manifest = json.loads((ROOT / ".release-please-manifest.json").read_text())
    tags = validate_release_please_tags.validate_tags(
        repo_root=tmp_path, config=config, manifest=manifest, target="HEAD"
    )
    assert all(not tag.startswith(TAG_PREFIX) for tag in tags)


def test_no_perception_tag_release_or_publication_route_is_registered():
    release_config = (ROOT / "release-please-config.json").read_text()
    release_manifest = (ROOT / ".release-please-manifest.json").read_text()
    release_workflow = (ROOT / ".github/workflows/release-please.yml").read_text()
    resolver = (ROOT / ".github/scripts/resolve_release_please_request.py").read_text()

    assert TAG_PREFIX not in release_config
    assert TAG_PREFIX not in release_manifest
    assert "cua-perception" not in release_workflow
    assert "cua-perception" not in resolver
    ci = (ROOT / ".github/workflows/ci-test-scripts.yml").read_text()
    assert "release_please_perception_preview.cjs" in ci
    assert "release-please@17.3.0" in ci
    preview = (ROOT / ".github/scripts/tests/release_please_perception_preview.cjs").read_text()
    assert "feat: add offline visual parsing" in preview
    assert "revert: restore previous protocol" in preview
    assert "libs/cua-driver/rust/crates/cua-driver/src/main.rs" in preview

    allowed_exclusion_workflows = {
        "cd-rust-cua-driver.yml",
        "ci-release-metadata.yml",
    }
    for workflow in (ROOT / ".github/workflows").glob("*.y*ml"):
        text = workflow.read_text()
        assert TAG_PREFIX not in text, workflow.name
        if workflow.name not in allowed_exclusion_workflows:
            assert "cua-perception" not in text, workflow.name

    for script in (ROOT / ".github/scripts").glob("*.py"):
        assert TAG_PREFIX not in script.read_text(), script.name
