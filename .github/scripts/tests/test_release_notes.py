"""Release Please components get Cua Driver/Lume release notes (CUA-1531)."""

from __future__ import annotations

import json
from pathlib import Path
import re

import pytest

import release_notes
import resolve_release_please_request
import validate_release_versions
from github_release import ReleaseError


ROOT = Path(__file__).resolve().parents[3]
CONFIG = json.loads((ROOT / "release-please-config.json").read_text())
MANIFEST = json.loads((ROOT / ".release-please-manifest.json").read_text())
WORKFLOWS = ROOT / ".github/workflows"
ENROLLED = {
    "libs/python/agent": "agent",
    "libs/python/core": "core",
    "libs/cua-bench": "bench",
    "libs/python/bench-ui": "bench-ui",
    "libs/python/som": "som",
    "libs/python/cua-sandbox-apps": "sandbox-apps",
    "libs/typescript/core": "npm-core",
    "libs/typescript/playground": "npm-playground",
    "libs/cuabot": "cuabot",
}


def components() -> dict[str, str]:
    return {package["component"]: path for path, package in CONFIG["packages"].items()}


@pytest.mark.parametrize(
    ("tag", "component", "paths", "excludes"),
    [
        ("cua-sdk-v0.4.2", "cua-sdk", ["libs/cua"], ["libs/cua/proto/MEDIA.md", "libs/cua/README.md"]),
        # additional-paths is not a release-please 17.3.0 option: the changelog
        # covers the package path only, so the notes do too.
        ("cua-spaces-v0.8.0", "cua-spaces", ["apps/cua-spaces-macos"], []),
        ("cua-spacesd-v0.5.4", "cua-spacesd", ["libs/cua-spacesd"], None),
        ("sandbox-v0.9.1", "sandbox", ["libs/python/cua-sandbox"], []),
        ("sandbox-apps-v0.1.3", "sandbox-apps", ["libs/python/cua-sandbox-apps"], []),
        ("bench-v0.3.1", "bench", ["libs/cua-bench"], []),
        ("bench-ui-v0.7.2", "bench-ui", ["libs/python/bench-ui"], []),
        ("core-v0.3.3", "core", ["libs/python/core"], []),
        ("npm-core-v0.1.7", "npm-core", ["libs/typescript/core"], []),
        ("cuabot-v1.0.17", "cuabot", ["libs/cuabot"], None),
    ],
)
def test_stable_tags_resolve_to_exactly_their_release_please_package(tag, component, paths, excludes):
    plan = release_notes.resolve_tag(CONFIG, tag)
    assert plan["component"] == component
    assert plan["paths"] == paths
    assert not plan["self_rendered"]
    assert plan["changelog"] == f"{paths[0]}/CHANGELOG.md"
    if excludes is not None:
        assert plan["exclude_paths"] == excludes


def test_nested_release_please_packages_are_excluded_from_their_parent():
    plan = release_notes.resolve_tag(CONFIG, "cua-driver-rs-v0.35.3")
    assert "libs/cua-driver/rust/crates/cua-perception" in plan["exclude_paths"]
    assert plan["self_rendered"]


@pytest.mark.parametrize(
    "tag",
    [
        "cua-spaces-v0.8.0-beta.6",  # betas keep the notes their CD workflow writes
        "docker-cuabot-v1.0.7",  # Docker images stay on the legacy bump
        "nightly-lume-v0.6.2-nightly.20261009.1",
        "fleet-v0.1.18",
        "cua-spaces-latest",
    ],
)
def test_prerelease_and_unmanaged_tags_are_not_rendered(tag):
    assert release_notes.resolve_tag(CONFIG, tag) is None


def test_every_component_has_one_owner_for_its_release_notes():
    triggers = re.findall(r'^      - "([^"]+)-v\*"$', (WORKFLOWS / "release-notes.yml").read_text(), re.M)
    for component in components():
        if component in release_notes.SELF_RENDERED:
            assert component not in triggers
        else:
            assert component in release_notes.DISPLAY_NAMES
            assert component in triggers, f"release-notes.yml does not run for {component} tags"
    assert set(triggers) <= set(components())


def test_enrolled_packages_keep_their_legacy_tag_and_current_version():
    for path, component in ENROLLED.items():
        package = CONFIG["packages"][path]
        assert package["component"] == component
        assert package["release-type"] == "simple"
        assert package["bump-minor-pre-major"] is True
        pyproject = ROOT / path / "pyproject.toml"
        if pyproject.exists():
            current = re.search(r'^version = "([^"]+)"$', pyproject.read_text(), re.M).group(1)
        else:
            current = json.loads((ROOT / path / "package.json").read_text())["version"]
        assert MANIFEST[path] == current
        assert not (ROOT / path / ".bumpversion.cfg").exists()


def test_legacy_bump_workflows_no_longer_release_enrolled_packages():
    services = ["pypi/agent", "pypi/bench", "pypi/bench-ui", "pypi/core", "pypi/sandbox-apps",
                "pypi/som", "npm/core", "npm/playground", "npm/cuabot"]
    for workflow in ("release-bump-version.yml", "release-on-merge.yml",
                     "ci-release-reminder.yml", "release-unreleased-digest.yml"):
        text = (WORKFLOWS / workflow).read_text()
        for service in services:
            assert f'"{service}"' not in text and f"- {service}\n" not in text, (workflow, service)
    # Docker images (libs/cuabot's container among them) stay on the legacy bump.
    assert "          - docker/cuabot\n" in (WORKFLOWS / "release-bump-version.yml").read_text()
    assert (ROOT / "libs/cuabot/.bumpversion.docker.cfg").exists()


def test_targeted_release_requests_cover_every_component():
    assert resolve_release_please_request.COMPONENT_PATHS == components()
    workflow = (WORKFLOWS / "release-please.yml").read_text()
    for component in components():
        assert f"          - {component}\n" in workflow


def test_install_footer_names_the_released_version():
    footer = release_notes.install_footer("cua-sdk", "0.4.2")
    assert 'pip install "cua==0.4.2"' in footer
    assert "npm install @trycua/cua@0.4.2" in footer
    assert release_notes.install_footer("cua-spaces", "0.8.0") == ""


class FakeApi:
    def __init__(self, releases):
        self.releases = releases
        self.patches = []

    def get(self, path):
        assert "/releases?per_page=100" in path
        return self.releases

    def patch(self, path, body):
        self.patches.append((path, body))
        return {}


def test_apply_replaces_only_the_body_of_a_draft():
    api = FakeApi([{"id": 7, "tag_name": "cua-sdk-v0.4.2", "draft": True, "body": "rp"}])
    assert release_notes.apply(api, "trycua/cua", "cua-sdk-v0.4.2", "rendered", attempts=1, delay=0)
    assert api.patches == [("repos/trycua/cua/releases/7", {"body": "rendered"})]


def test_apply_is_idempotent_and_refuses_a_missing_or_duplicated_release():
    same = FakeApi([{"id": 7, "tag_name": "core-v0.3.3", "draft": False, "body": "rendered"}])
    assert not release_notes.apply(same, "trycua/cua", "core-v0.3.3", "rendered", attempts=1, delay=0)
    assert same.patches == []
    with pytest.raises(ReleaseError, match="no GitHub release"):
        release_notes.apply(FakeApi([]), "trycua/cua", "core-v0.3.3", "x", attempts=2, delay=0)
    twice = FakeApi([{"id": 1, "tag_name": "core-v0.3.3"}, {"id": 2, "tag_name": "core-v0.3.3"}])
    with pytest.raises(ReleaseError, match="expected one"):
        release_notes.apply(twice, "trycua/cua", "core-v0.3.3", "x", attempts=1, delay=0)


def test_publish_draft_keeps_the_body_and_never_takes_latest():
    api = FakeApi([{"id": 9, "tag_name": "agent-v0.9.1", "draft": True, "body": "changelog"}])
    assert release_notes.publish_draft(
        api, "trycua/cua", "agent-v0.9.1", asset_dir=None, prerelease=False, name="cua-agent v0.9.1"
    )
    assert api.patches == [(
        "repos/trycua/cua/releases/9",
        {"draft": False, "prerelease": False, "make_latest": "false", "name": "cua-agent v0.9.1"},
    )]
    published = FakeApi([{"id": 9, "tag_name": "agent-v0.9.1", "draft": False}])
    assert release_notes.publish_draft(published, "trycua/cua", "agent-v0.9.1", asset_dir=None, prerelease=False)
    assert published.patches == []
    # No draft: the legacy workflow creates the release as before.
    assert not release_notes.publish_draft(FakeApi([]), "trycua/cua", "agent-v0.9.1", asset_dir=None, prerelease=False)


def test_version_check_catches_a_package_file_that_drifted(tmp_path):
    (tmp_path / "libs/python/core").mkdir(parents=True)
    (tmp_path / "libs/python/core/pyproject.toml").write_text('[project]\nname = "cua-core"\nversion = "0.3.3"\n')
    (tmp_path / "uv.lock").write_text('[[package]]\nname = "cua-core"\nversion = "0.3.2"\n')
    package = CONFIG["packages"]["libs/python/core"]
    values = {
        extra["path"]: validate_release_versions.extra_file_version(tmp_path, "libs/python/core", extra)
        for extra in package["extra-files"]
        if not extra["path"].startswith("/libs/")
    }
    assert values == {"pyproject.toml": "0.3.3", "/uv.lock": "0.3.2"}
    with pytest.raises(validate_release_versions.VersionError, match="mismatched"):
        validate_release_versions.require_equal("core", "0.3.3", values)


def test_legacy_baselines_are_exactly_the_enrolled_packages_last_legacy_tags():
    # One baseline per enrolled package, named for its last legacy tag. Until the
    # package's first Release Please release that is the manifest version; the
    # release PR then moves the manifest ahead of it (the baseline must stay for
    # the notes of that release), so the baseline may be older, never newer.
    by_component = {}
    for tag in release_notes.LEGACY_BASELINE_TAGS:
        component, version = tag.rsplit("-v", 1)
        assert component not in by_component, f"two legacy baselines for {component}"
        by_component[component] = tuple(int(part) for part in version.split("."))
    assert set(by_component) == set(ENROLLED.values())
    for path, component in ENROLLED.items():
        released = tuple(int(part) for part in MANIFEST[path].split("."))
        assert by_component[component] <= released, f"{component} baseline is newer than its manifest version"


def test_only_a_legacy_baseline_relaxes_the_changelog_citation_check(monkeypatch, tmp_path):
    seen = {}

    def fake_build(plan, repo_root, repository, tag, previous, sha, attribution, client, ref):
        seen[previous] = release_notes.release_attribution.changelog_references_change("", 1, [])
        return {"changes": [], "contributors": [], "displayName": "x", "version": plan["version"],
                "repository": repository, "tag": tag, "previousTag": previous, "compareUrl": "u"}

    original = release_notes.release_attribution.changelog_references_change
    monkeypatch.setattr(release_notes, "_build", fake_build)
    for previous in ("sandbox-apps-v0.1.2", "sandbox-apps-v0.1.3"):
        monkeypatch.setattr(release_notes.release_attribution, "find_previous_tag", lambda *_a, p=previous: p)
        release_notes.render(repo_root=ROOT, repository="trycua/cua", tag="sandbox-apps-v0.1.4",
                             sha="a" * 40, out_dir=tmp_path, ref=None, github=object())
        assert release_notes.release_attribution.changelog_references_change is original
    assert seen == {"sandbox-apps-v0.1.2": True, "sandbox-apps-v0.1.3": False}
