from __future__ import annotations

import json
from pathlib import Path
import subprocess

import pytest
from jsonschema import Draft202012Validator

from release_attribution import (
    CommitRecord,
    LEGACY_RELEASE_BUMP_RE,
    PUBLISHED_INSTALLER_BUMP_RE,
    ReleaseError,
    _change_contributors,
    build_manifest,
    changelog_references_change,
    linked_issue_numbers,
    login_from_email,
    merge_contributors,
    release_bump,
    release_entries,
    render_body,
    render_card_svg,
    render_card_alt_text,
    render_social,
    resolve_pull_for_commit,
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
    assert release_entries("feat(driver): add policies (#2235)", "", "")[0].summary == (
        "add policies"
    )


def test_release_tracked_changes_require_a_releasing_title_or_explicit_opt_out():
    for title in (
        "fix(cua-driver): preserve browser attachment",
        "feat(lume): add resumable pulls",
        "perf(cua-driver): reduce snapshot latency",
        "revert(lume): restore prior network setup",
    ):
        validate_pr_title(title, require_release=True)

    with pytest.raises(ReleaseError, match="makes Release Please skip"):
        validate_pr_title(
            "test(cua-driver): harden browser certification",
            require_release=True,
        )

    validate_pr_title(
        "style(cua-driver): apply deterministic formatting",
        require_release=True,
        allow_non_release=True,
    )


def test_login_from_noreply_and_override():
    assert login_from_email("123+octo-user@users.noreply.github.com", {}) == "octo-user"
    assert (
        login_from_email("github-actions[bot]@users.noreply.github.com", {})
        == "github-actions[bot]"
    )
    assert login_from_email("person@example.com", {"person@example.com": "person"}) == "person"
    assert login_from_email("person@example.com", {}) is None


def test_release_notes_dedupe_contributors_across_roles_and_login_case():
    contributors = [
        {"login": "ngnichtel", "role": "author", "external": True},
        {"login": "ngnichtel", "role": "coauthor", "external": True},
        {"login": "goldenfish123321", "role": "coauthor", "external": True},
        {"login": "GoldenFish123321", "role": "reporter", "external": True},
    ]
    assert merge_contributors(contributors) == [
        {
            "login": "goldenfish123321",
            "roles": ["coauthor", "reporter"],
            "external": True,
        },
        {
            "login": "ngnichtel",
            "roles": ["author", "coauthor"],
            "external": True,
        },
    ]

    manifest = {
        "displayName": "Cua Driver",
        "version": "0.13.1",
        "repository": "trycua/cua",
        "tag": "cua-driver-rs-v0.13.1",
        "compareUrl": (
            "https://github.com/trycua/cua/compare/cua-driver-rs-v0.12.6...cua-driver-rs-v0.13.1"
        ),
        "visualRequested": False,
        "changes": [
            {
                "type": "fix",
                "summary": "preserve contributor credit",
                "pr": 2559,
                "contributors": contributors,
            }
        ],
        "contributors": merge_contributors(contributors),
    }
    body = render_body(manifest)
    assert "Thanks @ngnichtel, @goldenfish123321." in body
    assert "reported by @GoldenFish123321" not in body
    assert body.count("@ngnichtel") == 2
    assert body.count("@goldenfish123321") == 2
    assert "@GoldenFish123321" not in body


def test_cua_driver_release_footer_explains_github_prerelease_label():
    footer = (REPO_ROOT / ".github/release-notes/cua-driver-rs.md").read_text()
    manifest = {
        "displayName": "Cua Driver",
        "version": "0.17.0",
        "repository": "trycua/cua",
        "tag": "cua-driver-rs-v0.17.0",
        "compareUrl": (
            "https://github.com/trycua/cua/compare/cua-driver-rs-v0.16.0...cua-driver-rs-v0.17.0"
        ),
        "visualRequested": False,
        "changes": [],
        "contributors": [],
        "assets": [],
    }

    body = render_body(manifest, footer)
    normalized = " ".join(body.split())

    assert "Why GitHub says “Pre-release”" in body
    assert "repository-wide “Latest” pointer" in normalized
    assert "plain Cua Driver SemVer" in normalized
    assert "0.17.0" not in footer
    assert "npm and PyPI" in normalized
    assert "`cua-driver-rs-v*` releases directly" in normalized


def test_cross_repository_references_are_not_resolved_in_cua():
    body = (
        "Closes https://github.com/other/project/issues/7 and closes #8. "
        "Salvaged from https://github.com/other/project/pull/9 and source-pr: #10"
    )
    assert linked_issue_numbers(body, "trycua/cua") == [8]
    assert source_pull_numbers(body, "trycua/cua") == [10]


def test_exact_squash_title_resolves_when_commit_association_is_missing():
    class UnassociatedGitHub:
        def pulls_for_commit(self, repository: str, commit_sha: str):
            return []

        def pull(self, repository: str, number: int):
            assert repository == "trycua/cua"
            assert number == 2235
            return {
                "number": 2235,
                "title": "feat(driver): add YAML and Rego permission policies",
                "merged_at": "2026-07-15T23:19:09Z",
            }

    commit = CommitRecord(
        "abc123",
        "feat(driver): add YAML and Rego permission policies (#2235)",
        "",
    )
    pull = resolve_pull_for_commit(UnassociatedGitHub(), "trycua/cua", commit)
    assert pull["number"] == 2235

    with pytest.raises(ReleaseError, match="not associated"):
        resolve_pull_for_commit(
            UnassociatedGitHub(),
            "trycua/cua",
            CommitRecord("def456", "feat(driver): no pull suffix", ""),
        )
    assert (
        resolve_pull_for_commit(
            UnassociatedGitHub(),
            "trycua/cua",
            CommitRecord("def456", "test(driver): no pull suffix", ""),
            required=False,
        )
        is None
    )


def test_merged_pr_override_recovers_a_non_releasing_squash_title(tmp_path: Path):
    git(tmp_path, "init")
    git(tmp_path, "config", "user.name", "Release Test")
    git(tmp_path, "config", "user.email", "release@example.com")
    product = tmp_path / "libs/cua-driver/rust"
    product.mkdir(parents=True)
    (product / "CHANGELOG.md").write_text("# Changelog\n")
    (product / "driver.txt").write_text("initial\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "chore: seed fixture")
    git(tmp_path, "tag", "cua-driver-rs-v0.9.0")

    (product / "driver.txt").write_text("hardened\n")
    (product / "CHANGELOG.md").write_text(
        "# Changelog\n\n"
        "## [0.9.1] (2026-07-20)\n\n"
        "* harden exact-profile browser attachment (#2367)\n"
    )
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "test(cua-driver): harden browser certification (#2367)")
    release_sha = git(tmp_path, "rev-parse", "HEAD")
    git(tmp_path, "tag", "cua-driver-rs-v0.9.1")

    class OverrideGitHub:
        def pulls_for_commit(self, repository: str, commit_sha: str):
            assert repository == "trycua/cua"
            assert commit_sha == release_sha
            return [{"number": 2367, "merge_commit_sha": commit_sha, "merged_at": "now"}]

        def pull(self, repository: str, number: int):
            assert repository == "trycua/cua"
            assert number == 2367
            return {
                "number": 2367,
                "user": {"login": "browser-author"},
                "author_association": "MEMBER",
                "body": (
                    "BEGIN_COMMIT_OVERRIDE\n"
                    "fix(cua-driver): harden exact-profile browser attachment\n\n"
                    "fix(cua-driver): fail closed for unsupported browser routes\n"
                    "END_COMMIT_OVERRIDE"
                ),
                "labels": [],
            }

        def issue(self, repository: str, number: int):
            raise AssertionError("the override fixture does not reference issues")

    manifest = build_manifest(
        repo_root=tmp_path,
        repository="trycua/cua",
        product="cua-driver-rs",
        display_name="Cua Driver",
        version="0.9.1",
        tag="cua-driver-rs-v0.9.1",
        previous_tag="cua-driver-rs-v0.9.0",
        expected_sha=release_sha,
        paths=("libs/cua-driver",),
        changelog_path=product / "CHANGELOG.md",
        attribution_config={
            "bots": [],
            "coauthorOverrides": {},
            "ignoredCoauthorEmails": [],
            "identityOverrides": {},
            "internalHandles": ["browser-author"],
            "optOutHandles": [],
        },
        github=OverrideGitHub(),
    )

    assert [
        (change["type"], change["summary"], change["pr"]) for change in manifest["changes"]
    ] == [
        ("fix", "harden exact-profile browser attachment", 2367),
        ("fix", "fail closed for unsupported browser routes", 2367),
    ]


def test_legacy_release_bump_subject_is_recognized():
    assert LEGACY_RELEASE_BUMP_RE.match("Bump cua-driver-rs to v0.8.3")
    assert LEGACY_RELEASE_BUMP_RE.match("Bump lume to v0.3.16")
    assert not LEGACY_RELEASE_BUMP_RE.match("feat(driver): bump reconnect retries")


def test_published_installer_bump_subject_is_recognized_narrowly():
    assert PUBLISHED_INSTALLER_BUMP_RE.match(
        "chore(cua-driver): advance published installer version to 0.19.3 [skip ci]"
    )
    assert not PUBLISHED_INSTALLER_BUMP_RE.match(
        "chore(cua-driver): advance published installer version to nightly [skip ci]"
    )
    assert not PUBLISHED_INSTALLER_BUMP_RE.match(
        "feat(cua-driver): advance published installer version to 0.19.3 [skip ci]"
    )


def test_changelog_accepts_verified_commit_link_when_pr_suffix_is_missing():
    commit_sha = "2dad3e519e17b27eaa793151b8671957f578072c"
    section = (
        "## [0.11.0] (2026-07-22)\n\n"
        "* **cua-driver:** add persistent sessions "
        f"([{commit_sha[:7]}](https://github.com/trycua/cua/commit/{commit_sha}))\n"
    )

    assert changelog_references_change(section, 2339, [commit_sha])
    assert changelog_references_change(section + "* fix ([#2408](url))\n", 2408, [])
    assert not changelog_references_change(section, 9999, ["f" * 40])


def test_breaking_change_remains_minor_before_one_dot_zero():
    changes = [{"type": "feat", "breaking": True}]

    assert release_bump(changes, "0.11.0") == "minor"
    assert release_bump(changes, "1.0.0") == "major"
    with pytest.raises(ReleaseError, match="not semantic"):
        release_bump(changes, "next")


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


def test_nightly_manifest_attributes_maintenance_prs_without_a_versioned_changelog(
    tmp_path: Path,
):
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

    (product / "driver.txt").write_text("documented\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-m", "docs(driver): explain nightly sessions")
    commit_sha = git(tmp_path, "rev-parse", "HEAD")

    config = {
        "bots": [],
        "coauthorOverrides": {},
        "ignoredCoauthorEmails": [],
        "identityOverrides": {},
        "internalHandles": [],
        "optOutHandles": [],
    }
    manifest = build_manifest(
        repo_root=tmp_path,
        repository="trycua/cua",
        product="cua-driver-rs",
        display_name="Cua Driver",
        version="0.8.2-nightly.20260812.42",
        tag="nightly-cua-driver-rs-v0.8.2-nightly.20260812.42",
        release_ref=commit_sha,
        previous_tag="cua-driver-rs-v0.8.1",
        expected_sha=commit_sha,
        paths=("libs/cua-driver",),
        changelog_path=product / "CHANGELOG.md",
        attribution_config=config,
        github=FakeGitHub(commit_sha),
        channel="nightly",
    )

    assert manifest["channel"] == "nightly"
    assert manifest["compareUrl"].endswith(f"/compare/cua-driver-rs-v0.8.1...{commit_sha}")
    assert manifest["visualRequested"] is False
    assert manifest["changes"][0]["type"] == "docs"
    assert manifest["changes"][0]["pr"] == 12
    assert {item["login"] for item in manifest["contributors"]} == {
        "bug-reporter",
        "pr-author",
        "source-author",
    }
    schema = json.loads((REPO_ROOT / ".github/release-manifest.schema.json").read_text())
    validator = Draft202012Validator(schema, format_checker=None)
    validator.validate(manifest)
    stable_shaped = dict(manifest)
    stable_shaped.pop("channel")
    assert any("is not one of" in error.message for error in validator.iter_errors(stable_shaped))

    with pytest.raises(ReleaseError, match="no releasing pull requests"):
        build_manifest(
            repo_root=tmp_path,
            repository="trycua/cua",
            product="cua-driver-rs",
            display_name="Cua Driver",
            version="0.8.2",
            tag="cua-driver-rs-v0.8.2",
            release_ref=commit_sha,
            previous_tag="cua-driver-rs-v0.8.1",
            expected_sha=commit_sha,
            paths=("libs/cua-driver",),
            changelog_path=product / "CHANGELOG.md",
            attribution_config=config,
            github=FakeGitHub(commit_sha),
        )


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


def test_pr_2805_coauthor_resolves_through_trusted_identity_override():
    config = json.loads((REPO_ROOT / ".github/release-attribution-config.json").read_text())
    commit = CommitRecord(
        "aebd9962d2686e75ff9e17e0a3735e303ff96981",
        "fix(cua-driver): stop Windows update --apply from killing itself (#2805)",
        "Co-authored-by: Roman Syuzyov <rsyuzyov@gmail.com>",
    )
    pull = {
        "user": {"login": "rsyuzyov"},
        "author_association": "CONTRIBUTOR",
        "body": "",
        "labels": [],
    }

    contributors, issues, visual_requested = _change_contributors(
        pull,
        commit,
        FakeGitHub(commit.sha),
        "trycua/cua",
        config,
    )

    assert contributors == [
        {"login": "rsyuzyov", "role": "author", "external": True},
        {"login": "rsyuzyov", "role": "coauthor", "external": True},
    ]
    assert issues == []
    assert visual_requested is False


def test_pr_3266_squash_coauthor_resolves_through_verified_identity_override():
    config = json.loads((REPO_ROOT / ".github/release-attribution-config.json").read_text())
    commit = CommitRecord(
        "2fd8bfc6dd5d7d67d00a4151c1159e665abb9ef0",
        "test(cua-driver): seed macOS Lume TCC grants (#3266)",
        "Co-authored-by: jf-mac-mini <jf-mac-mini@jf-mac-mini-4.local>",
    )
    pull = {
        "user": {"login": "0xjohnnydev"},
        "author_association": "CONTRIBUTOR",
        "body": "",
        "labels": [],
    }

    contributors, _, _ = _change_contributors(
        pull,
        commit,
        FakeGitHub(commit.sha),
        "trycua/cua",
        config,
    )
    assert contributors == [
        {"login": "0xjohnnydev", "role": "author", "external": True},
        {"login": "0xjohnnydev", "role": "coauthor", "external": True},
    ]
