from __future__ import annotations

import json
from pathlib import Path
import subprocess

import pytest
from jsonschema import Draft202012Validator

from release_attribution import (
    CommitRecord,
    ReleaseError,
    _change_contributors,
    build_manifest,
    linked_issue_numbers,
    login_from_email,
    release_entries,
    render_body,
    render_card_svg,
    render_card_alt_text,
    render_social,
    source_pull_numbers,
    validate_pr_title,
)


REPO_ROOT = Path(__file__).resolve().parents[3]


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, text=True, capture_output=True
    ).stdout.strip()


class FakeGitHub:
    def __init__(self, commit_sha: str) -> None:
        self.commit_sha = commit_sha

    def pulls_for_commit(self, repository: str, commit_sha: str):
        assert repository == "trycua/cua"
        assert commit_sha == self.commit_sha
        return [{"number": 12, "merge_commit_sha": commit_sha, "merged_at": "now"}]

    def pull(self, repository: str, number: int):
        if number == 9:
            return {
                "number": 9,
                "user": {"login": "source-author"},
                "author_association": "NONE",
                "body": "",
                "labels": [],
            }
        assert number == 12
        return {
            "number": 12,
            "user": {"login": "pr-author"},
            "author_association": "NONE",
            "body": "Closes #7\n\nSalvaged from #9",
            "labels": [{"name": "release-visual"}],
        }

    def issue(self, repository: str, number: int):
        assert number == 7
        return {
            "number": 7,
            "user": {"login": "bug-reporter"},
            "author_association": "NONE",
        }


def test_title_validation_and_override_entries():
    validate_pr_title("feat(driver)!: expose structured reconnect state")
    with pytest.raises(ReleaseError, match="Conventional Commit"):
        validate_pr_title("Make reconnect better")
    with pytest.raises(ReleaseError, match="unsupported"):
        validate_pr_title("release(driver): reconnect")

    entries = release_entries(
        "chore: merge work",
        "",
        "BEGIN_COMMIT_OVERRIDE\nfeat(driver): add readiness\nfix(driver): keep focus\nEND_COMMIT_OVERRIDE",
    )
    assert [(entry.change_type, entry.summary) for entry in entries] == [
        ("feat", "add readiness"),
        ("fix", "keep focus"),
    ]


def test_login_from_noreply_and_override():
    assert login_from_email("123+octo-user@users.noreply.github.com", {}) == "octo-user"
    assert (
        login_from_email("github-actions[bot]@users.noreply.github.com", {})
        == "github-actions[bot]"
    )
    assert login_from_email("person@example.com", {"person@example.com": "person"}) == "person"
    assert login_from_email("person@example.com", {}) is None


def test_cross_repository_references_are_not_resolved_in_cua():
    body = (
        "Closes https://github.com/other/project/issues/7 and closes #8. "
        "Salvaged from https://github.com/other/project/pull/9 and source-pr: #10"
    )
    assert linked_issue_numbers(body, "trycua/cua") == [8]
    assert source_pull_numbers(body, "trycua/cua") == [10]


def test_manifest_is_pr_first_and_renders_deterministically(tmp_path: Path):
    git(tmp_path, "init")
    git(tmp_path, "config", "user.name", "Release Test")
    git(tmp_path, "config", "user.email", "release@example.com")
    product = tmp_path / "libs/cua-driver/rust"
    product.mkdir(parents=True)
    (product / "CHANGELOG.md").write_text("# Changelog\n")
    (product / "driver.txt").write_text("initial\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "chore: seed fixture")
    git(tmp_path, "tag", "cua-driver-rs-v0.8.1")

    python = tmp_path / "libs/cua-driver/python"
    python.mkdir(parents=True)
    (python / "wrapper.py").write_text("excluded\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "fix(python): excluded wrapper change")

    (product / "driver.txt").write_text("fixed\n")
    (product / "CHANGELOG.md").write_text(
        "# Changelog\n\n"
        "## [0.8.2](https://github.com/trycua/cua/compare/"
        "cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2) (2026-07-16)\n\n"
        "* fix: preserve focus (#12)\n"
    )
    git(tmp_path, "add", ".")
    git(
        tmp_path,
        "commit",
        "-m",
        "fix(driver): preserve focus while reconnecting",
        "-m",
        "Co-authored-by: Claude Opus <noreply@anthropic.com>\n"
        "Co-authored-by: Actions <github-actions[bot]@users.noreply.github.com>\n"
        "Co-authored-by: Pair <123+pair-user@users.noreply.github.com>",
    )
    commit_sha = git(tmp_path, "rev-parse", "HEAD")
    git(tmp_path, "tag", "cua-driver-rs-v0.8.2")

    assets = tmp_path / "assets"
    assets.mkdir()
    (assets / "cua-driver").write_bytes(b"binary")
    manifest = build_manifest(
        repo_root=tmp_path,
        repository="trycua/cua",
        product="cua-driver-rs",
        display_name="Cua Driver",
        version="0.8.2",
        tag="cua-driver-rs-v0.8.2",
        previous_tag="cua-driver-rs-v0.8.1",
        expected_sha=commit_sha,
        paths=("libs/cua-driver",),
        exclude_paths=("libs/cua-driver/python",),
        changelog_path=product / "CHANGELOG.md",
        attribution_config={
            "bots": ["github-actions[bot]"],
            "coauthorOverrides": {},
            "ignoredCoauthorEmails": ["noreply@anthropic.com"],
            "identityOverrides": {},
            "internalHandles": [],
            "optOutHandles": [],
        },
        github=FakeGitHub(commit_sha),
        asset_dir=assets,
    )

    assert manifest["repository"] == "trycua/cua"
    assert manifest["schema"].endswith(f"/{commit_sha}/.github/release-manifest.schema.json")
    assert manifest["bump"] == "patch"
    assert manifest["visualRequested"] is True
    assert manifest["changes"] == [
        {
            "type": "fix",
            "scope": "driver",
            "summary": "preserve focus while reconnecting",
            "breaking": False,
            "pr": 12,
            "issues": [7],
            "contributors": [
                {"login": "pr-author", "role": "author", "external": True},
                {"login": "pair-user", "role": "coauthor", "external": True},
                {"login": "source-author", "role": "coauthor", "external": True},
                {"login": "bug-reporter", "role": "reporter", "external": True},
            ],
        }
    ]
    assert manifest["assets"][0]["name"] == "cua-driver"
    schema = json.loads((REPO_ROOT / ".github/release-manifest.schema.json").read_text())
    Draft202012Validator(schema, format_checker=None).validate(manifest)
    body = render_body(manifest)
    assert "Thanks @pr-author, @pair-user, @source-author; reported by @bug-reporter." in body
    assert "https://github.com/trycua/cua/pull/12" in body
    assert "releases/download/cua-driver-rs-v0.8.2/release-card.png" in body
    social = render_social(manifest)
    assert len(social.rstrip()) <= 280
    assert "releases/tag/cua-driver-rs-v0.8.2" in social
    assert "Thanks to 4 community contributors." in social
    assert "@pr-author" not in social
    card = render_card_svg(manifest)
    assert "Cua Driver" in card
    assert "trycua/cua" in card
    alt = render_card_alt_text(manifest)
    assert alt.startswith("Cua Driver 0.8.2 release highlights:")
    assert "4 external contributors" in alt
    assert json.dumps(manifest, sort_keys=True) == json.dumps(manifest, sort_keys=True)

    preflight = build_manifest(
        repo_root=tmp_path,
        repository="trycua/cua",
        product="cua-driver-rs",
        display_name="Cua Driver",
        version="0.8.2",
        tag="cua-driver-rs-v0.8.2-not-created-yet",
        release_ref=commit_sha,
        previous_tag="cua-driver-rs-v0.8.1",
        expected_sha=commit_sha,
        paths=("libs/cua-driver",),
        exclude_paths=("libs/cua-driver/python",),
        changelog_path=product / "CHANGELOG.md",
        attribution_config={
            "bots": ["github-actions[bot]"],
            "coauthorOverrides": {},
            "ignoredCoauthorEmails": ["noreply@anthropic.com"],
            "identityOverrides": {},
            "internalHandles": [],
            "optOutHandles": [],
        },
        github=FakeGitHub(commit_sha),
        asset_dir=assets,
    )
    assert preflight["tag"] == "cua-driver-rs-v0.8.2-not-created-yet"
    assert preflight["sha"] == commit_sha


def test_unresolved_human_coauthor_fails_closed():
    commit = CommitRecord(
        "deadbeef",
        "fix(driver): preserve focus",
        "Co-authored-by: Unknown Person <private@example.com>",
    )
    pull = {
        "user": {"login": "author"},
        "author_association": "NONE",
        "body": "",
        "labels": [],
    }
    with pytest.raises(ReleaseError, match="unresolved human coauthor"):
        _change_contributors(
            pull,
            commit,
            FakeGitHub("deadbeef"),
            "trycua/cua",
            {
                "bots": [],
                "coauthorOverrides": {},
                "ignoredCoauthorEmails": [],
                "identityOverrides": {},
                "internalHandles": [],
                "optOutHandles": [],
            },
        )
