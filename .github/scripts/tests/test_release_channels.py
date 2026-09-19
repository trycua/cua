from __future__ import annotations

import ast
import json
from pathlib import Path
import shutil
import subprocess

import pytest
from jsonschema import Draft202012Validator

import github_release
import release_channels
from release_channels import (
    ChannelError,
    apply_version,
    build_manifest,
    component_descriptor,
    derive_nightly_version,
    format_tag,
    load_registry,
    parse_tag,
    plan_nightly,
    repository_path,
    render_nightly_body,
    stage_versioned_tree,
)


ROOT = Path(__file__).resolve().parents[3]
REGISTRY = ROOT / ".github/releases/components.json"


def test_release_channel_repository_io_is_explicitly_utf8():
    source_path = ROOT / ".github/scripts/release_channels.py"
    tree = ast.parse(source_path.read_text(encoding="utf-8"))
    calls = [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr in {"read_text", "write_text", "open"}
    ]
    assert calls
    for call in calls:
        encoding = next(
            (keyword.value for keyword in call.keywords if keyword.arg == "encoding"), None
        )
        assert isinstance(encoding, ast.Constant), ast.unparse(call)
        assert encoding.value == "utf-8", ast.unparse(call)


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, text=True, capture_output=True
    ).stdout.strip()


def test_registry_matches_release_please_and_channel_prefixes_are_disjoint():
    schema = json.loads((ROOT / ".github/releases/component.schema.json").read_text())
    Draft202012Validator(schema).validate(json.loads(REGISTRY.read_text()))
    registry = load_registry(REGISTRY, root=ROOT)
    assert set(registry["components"]) == {"cua-driver-rs", "lume"}
    all_prefixes = {
        component[key]
        for component in registry["components"].values()
        for key in ("stableTagPrefix", "nightlyTagPrefix")
    }
    assert len(all_prefixes) == 4


@pytest.mark.parametrize(
    ("component_name", "stable", "nightly"),
    [
        (
            "cua-driver-rs",
            "cua-driver-rs-v1.2.3",
            "nightly-cua-driver-rs-v1.2.3-nightly.20260812.42",
        ),
        ("lume", "lume-v1.2.3", "nightly-lume-v1.2.3-nightly.20260812.42"),
    ],
)
def test_strict_tag_grammars_do_not_cross_channels(component_name, stable, nightly):
    component = component_descriptor(component_name, REGISTRY, root=ROOT)
    assert parse_tag(component, "stable", stable) == "1.2.3"
    assert parse_tag(component, "nightly", nightly) == "1.2.3-nightly.20260812.42"
    with pytest.raises(ChannelError):
        parse_tag(component, "stable", nightly)
    with pytest.raises(ChannelError):
        parse_tag(component, "nightly", stable)


@pytest.mark.parametrize(
    "value",
    [
        "1.2.3-nightly.20261301.1",
        "1.2.3-nightly.20260812.0",
        "1.2.3-nightly.20260812.latest",
        "01.2.3-nightly.20260812.1",
        "1.2.3-beta.1",
    ],
)
def test_nightly_grammar_rejects_noncanonical_versions(value):
    with pytest.raises(ChannelError):
        release_channels.nightly_version(value)


def test_derivation_increments_patch_and_is_deterministic():
    assert derive_nightly_version("0.19.3", "20260812", "3097") == ("0.19.4-nightly.20260812.3097")


def copy_version_fixture(tmp_path: Path, component_name: str) -> tuple[Path, Path]:
    registry = json.loads(REGISTRY.read_text())
    wanted = {
        "release-please-config.json",
        ".release-please-manifest.json",
    }
    for component in registry["components"].values():
        wanted.update(
            {
                component["versionAuthorityFile"],
                component["changelog"],
                component["builderWorkflow"],
            }
        )
        for site in component["buildVersionSites"]:
            wanted.add(site["path"])
            if site["kind"] == "cargo-workspace-lock":
                wanted.add(site["manifestPath"])
                manifest_parent = ROOT / Path(site["manifestPath"]).parent
                for package_manifest in manifest_parent.glob("crates/*/Cargo.toml"):
                    wanted.add(str(package_manifest.relative_to(ROOT)))
    for relative in wanted:
        source = ROOT / relative
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    registry_path = tmp_path / ".github/releases/components.json"
    registry_path.parent.mkdir(parents=True, exist_ok=True)
    registry_path.write_text(json.dumps(registry))
    return registry_path, tmp_path


def test_driver_version_staging_updates_only_declared_build_sites(tmp_path: Path):
    registry, root = copy_version_fixture(tmp_path, "cua-driver-rs")
    changed = apply_version(
        "cua-driver-rs",
        "0.19.4-nightly.20260812.3097",
        registry_path=registry,
        root=root,
    )
    assert set(changed) == {
        "libs/cua-driver/rust/VERSION",
        "libs/cua-driver/rust/Cargo.toml",
        "libs/cua-driver/rust/Cargo.lock",
        "libs/cua-driver/rust/Skills/cua-driver/SKILL.md",
    }
    assert (root / "libs/cua-driver/rust/VERSION").read_text().strip().endswith(".3097")
    lock = (root / "libs/cua-driver/rust/Cargo.lock").read_text()
    assert 'name = "cua-driver"\nversion = "0.19.4-nightly.20260812.3097"' in lock
    assert 'name = "cursor-overlay"\nversion = "0.19.4-nightly.20260812.3097"' in lock
    assert 'name = "serde"\nversion = "0.19.4-nightly.20260812.3097"' not in lock


def test_lume_version_staging_preserves_stable_installer_default(tmp_path: Path):
    registry, root = copy_version_fixture(tmp_path, "lume")
    stable_installer = ROOT / "libs/lume/scripts/install.sh"
    destination = root / "libs/lume/scripts/install.sh"
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(stable_installer, destination)
    before = destination.read_text()
    apply_version(
        "lume",
        "0.5.4-nightly.20260812.3097",
        registry_path=registry,
        root=root,
    )
    assert "0.5.4-nightly.20260812.3097" in (root / "libs/lume/src/Main.swift").read_text()
    assert destination.read_text() == before


def test_first_nightly_plan_is_reproducible_and_uses_stable_attribution_base(
    monkeypatch: pytest.MonkeyPatch,
):
    source_sha = "a" * 40
    stable_version = (ROOT / "libs/lume/VERSION").read_text(encoding="utf-8").strip()
    stable_tag = f"lume-v{stable_version}"
    nightly_version = derive_nightly_version(stable_version, "20260812", "42")

    def fake_git(_root, command, *args):
        if command == "rev-list":
            assert args == ("-n", "1", stable_tag)
            return "d" * 40
        assert command == "merge-base"
        assert args == ("--is-ancestor", "d" * 40, source_sha)
        return ""

    monkeypatch.setattr(release_channels, "_git", fake_git)
    plan = plan_nightly(
        "lume",
        source_sha,
        "20260812",
        "42",
        [],
        registry_path=REGISTRY,
        root=ROOT,
    )
    assert plan["shouldBuild"] is True
    assert plan["reason"] == "first-nightly"
    assert plan["tag"] == f"nightly-lume-v{nightly_version}"
    assert plan["bundleVersion"] == nightly_version.split("-", 1)[0]
    assert plan["previousNightlyTag"] is None
    assert plan["attributionBaseTag"] == stable_tag


def test_plan_skips_an_identical_published_source(monkeypatch: pytest.MonkeyPatch):
    source_sha = "b" * 40
    monkeypatch.setattr(release_channels, "_git", lambda *_args: source_sha)
    plan = plan_nightly(
        "lume",
        source_sha,
        "20260812",
        "43",
        [
            {
                "tag_name": "nightly-lume-v0.5.4-nightly.20260811.41",
                "draft": False,
                "published_at": "2026-08-11T04:43:00Z",
            }
        ],
        registry_path=REGISTRY,
        root=ROOT,
    )
    assert plan["shouldBuild"] is False
    assert plan["reason"] == "source-unchanged"


def test_plan_builds_only_for_declared_relevant_changes(monkeypatch: pytest.MonkeyPatch):
    previous_sha = "b" * 40
    source_sha = "c" * 40

    def fake_git(_root, command, *args):
        if command == "rev-list":
            return previous_sha
        if command == "merge-base":
            assert args == ("--is-ancestor", previous_sha, source_sha)
            return ""
        assert command == "diff"
        assert "libs/cua-driver" in args
        return "libs/cua-driver/rust/crates/cua-driver/src/main.rs"

    monkeypatch.setattr(release_channels, "_git", fake_git)
    plan = plan_nightly(
        "cua-driver-rs",
        source_sha,
        "20260812",
        "44",
        [
            {
                "tag_name": "nightly-cua-driver-rs-v0.19.4-nightly.20260811.41",
                "draft": False,
                "published_at": "2026-08-11T04:17:00Z",
            }
        ],
        registry_path=REGISTRY,
        root=ROOT,
    )
    assert plan["shouldBuild"] is True
    assert plan["reason"] == "relevant-changes"
    assert plan["previousNightlyTag"] == ("nightly-cua-driver-rs-v0.19.4-nightly.20260811.41")
    assert plan["attributionBaseTag"] == plan["previousNightlyTag"]


def test_plan_holds_before_build_for_unresolved_attribution(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
):
    previous_sha = "b" * 40
    source_sha = "c" * 40
    config_path = tmp_path / "release-attribution-config.json"
    config_path.write_text(
        json.dumps(
            {
                "ignoredCoauthorEmails": [],
                "identityOverrides": {},
                "coauthorOverrides": {},
            }
        ),
        encoding="utf-8",
    )

    def fake_git(_root, command, *args):
        if command == "rev-list":
            return previous_sha
        if command == "merge-base":
            return ""
        if command == "diff":
            return "libs/cua-driver/rust/src/main.rs"
        raise AssertionError((command, args))

    monkeypatch.setattr(release_channels, "_git", fake_git)
    monkeypatch.setattr(
        release_channels.release_attribution,
        "commits_in_range",
        lambda *_args: [
            release_channels.release_attribution.CommitRecord(
                "deadbeef",
                "fix: preserve attribution",
                "Co-authored-by: Local Machine <machine@example.invalid>",
            )
        ],
    )
    plan = plan_nightly(
        "cua-driver-rs",
        source_sha,
        "20260812",
        "45",
        [
            {
                "tag_name": "nightly-cua-driver-rs-v0.19.4-nightly.20260811.41",
                "draft": False,
                "published_at": "2026-08-11T04:17:00Z",
            }
        ],
        registry_path=REGISTRY,
        root=ROOT,
        attribution_config_path=config_path,
    )
    assert plan["shouldBuild"] is False
    assert plan["reason"] == "held-attribution"
    assert plan["attributionIssues"] == [
        {"sha": "deadbeef", "name": "Local Machine", "email": "machine@example.invalid"}
    ]


def test_manifest_uses_stable_authority_with_a_separately_versioned_asset_tree(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
):
    registry, root = copy_version_fixture(tmp_path, "cua-driver-rs")
    git(root, "init")
    git(root, "config", "user.name", "Release Test")
    git(root, "config", "user.email", "release@example.com")
    git(root, "add", ".")
    git(root, "commit", "-m", "chore: seed fixture")
    git(root, "tag", "cua-driver-rs-v0.19.3")
    source_sha = git(root, "rev-parse", "HEAD")
    version = "0.19.4-nightly.20260812.42"
    source_skill = root / "libs/cua-driver/rust/Skills/cua-driver/SKILL.md"
    source_before = source_skill.read_text(encoding="utf-8")
    staged = root / "release-stage/cua-driver-skills"
    changed = stage_versioned_tree(
        "cua-driver-rs",
        version,
        "libs/cua-driver/rust/Skills/cua-driver",
        "release-stage/cua-driver-skills",
        registry_path=registry,
        root=root,
    )

    assert changed == ["release-stage/cua-driver-skills/SKILL.md"]
    assert source_skill.read_text(encoding="utf-8") == source_before
    assert version in (staged / "SKILL.md").read_text(encoding="utf-8")
    load_registry(registry, root=root)

    assets = root / "release-upload"
    assets.mkdir()
    (assets / "cua-driver.tar.gz").write_bytes(b"nightly")

    def fake_build_manifest(**kwargs):
        return {"version": kwargs["version"], "tag": kwargs["tag"]}

    monkeypatch.setattr(release_channels.release_attribution, "build_manifest", fake_build_manifest)
    manifest = build_manifest(
        "cua-driver-rs",
        version,
        f"nightly-cua-driver-rs-v{version}",
        source_sha,
        "cua-driver-rs-v0.19.3",
        assets,
        repository="trycua/cua",
        registry_path=registry,
        root=root,
        attribution_config_path=ROOT / ".github/release-attribution-config.json",
        github=object(),
    )
    assert manifest == {
        "version": version,
        "tag": f"nightly-cua-driver-rs-v{version}",
    }

    with pytest.raises(ChannelError, match="destination already exists"):
        stage_versioned_tree(
            "cua-driver-rs",
            version,
            "libs/cua-driver/rust/Skills/cua-driver",
            "release-stage/cua-driver-skills",
            registry_path=registry,
            root=root,
        )
    with pytest.raises(ChannelError, match="contains no declared buildVersionSites"):
        stage_versioned_tree(
            "cua-driver-rs",
            version,
            ".github",
            "release-stage/metadata",
            registry_path=registry,
            root=root,
        )


def test_nightly_manifest_reuses_pr_attribution_and_renders_contributors(tmp_path: Path):
    registry, root = copy_version_fixture(tmp_path, "lume")
    git(root, "init")
    git(root, "config", "user.name", "Release Test")
    git(root, "config", "user.email", "release@example.com")
    git(root, "add", ".")
    git(root, "commit", "-m", "chore: seed fixture")
    git(root, "tag", "lume-v0.5.3")
    main = root / "libs/lume/src/Main.swift"
    main.write_text(main.read_text() + "\n// Nightly install notes\n")
    git(root, "add", ".")
    git(root, "commit", "-m", "docs(lume): explain nightly installation")
    source_sha = git(root, "rev-parse", "HEAD")

    class NightlyGitHub:
        def pulls_for_commit(self, repository: str, commit_sha: str):
            assert repository == "trycua/cua"
            assert commit_sha == source_sha
            return [{"number": 42, "merge_commit_sha": commit_sha, "merged_at": "now"}]

        def pull(self, repository: str, number: int):
            assert repository == "trycua/cua"
            assert number == 42
            return {
                "number": 42,
                "user": {"login": "nightly-contributor"},
                "author_association": "NONE",
                "body": "",
                "labels": [],
            }

        def issue(self, repository: str, number: int):
            raise AssertionError("the nightly fixture does not reference issues")

    assets = root / "artifacts"
    assets.mkdir()
    (assets / "artifact.tar.gz").write_bytes(b"nightly")
    version = "0.5.4-nightly.20260812.42"
    component = component_descriptor("lume", registry, root=root)
    manifest = build_manifest(
        "lume",
        version,
        format_tag(component, "nightly", version),
        source_sha,
        "lume-v0.5.3",
        assets,
        repository="trycua/cua",
        registry_path=registry,
        root=root,
        attribution_config_path=ROOT / ".github/release-attribution-config.json",
        github=NightlyGitHub(),
    )
    assert manifest["channel"] == "nightly"
    schema = json.loads((ROOT / ".github/release-manifest.schema.json").read_text())
    Draft202012Validator(schema, format_checker=None).validate(manifest)
    assert manifest["changes"][0]["type"] == "docs"
    assert manifest["contributors"] == [
        {
            "login": "nightly-contributor",
            "roles": ["author"],
            "external": True,
        }
    ]
    assert manifest["assets"][0]["sha256"] == (
        "2a3b62b53ddb9f167b63d22202a360811ba78df015021f704d01ee9abad4169c"
    )
    body = render_nightly_body(manifest)
    assert "LUME_VERSION=nightly-lume-v0.5.4-nightly.20260812.42" in body
    assert "never replace stable" in body
    assert "[#42](https://github.com/trycua/cua/pull/42)" in body
    assert body.count("@nightly-contributor") == 2


class RehearsalGitHub:
    def __init__(self, source_sha: str, title: str) -> None:
        self.source_sha = source_sha
        self.title = title

    def pulls_for_commit(self, repository: str, commit_sha: str):
        assert repository == "trycua/cua"
        assert commit_sha == self.source_sha
        return [
            {
                "number": 42,
                "merge_commit_sha": commit_sha,
                "merged_at": "2026-08-12T12:00:00Z",
            }
        ]

    def pull(self, repository: str, number: int):
        assert repository == "trycua/cua"
        assert number == 42
        return {
            "number": 42,
            "title": self.title,
            "user": {"login": "nightly-contributor"},
            "author_association": "NONE",
            "body": "",
            "labels": [],
            "merge_commit_sha": self.source_sha,
            "merged_at": "2026-08-12T12:00:00Z",
        }

    def issue(self, repository: str, number: int):
        raise AssertionError(f"unexpected issue lookup for {repository}#{number}")


class RehearsalReleaseApi:
    def __init__(self, source_sha: str) -> None:
        self.source_sha = source_sha
        self.release = None
        self.assets = []
        self.posts = []
        self.patches = []
        self.uploads = []

    def get(self, path: str):
        if "/releases?" in path:
            return [self.release] if self.release else []
        if "/git/ref/tags/nightly-" in path:
            return {"object": {"type": "commit", "sha": self.source_sha}}
        if path.endswith("/assets?per_page=100&page=1"):
            return list(self.assets)
        if "/commits/" in path:
            return {"sha": path.rsplit("/", 1)[1]}
        raise AssertionError(path)

    def post(self, path: str, body: dict):
        self.posts.append((path, body))
        self.release = {
            "id": 8,
            "tag_name": body["tag_name"],
            "draft": body["draft"],
            "prerelease": body["prerelease"],
            "body": body["body"],
            "target_commitish": body["target_commitish"],
            "upload_url": "https://uploads.example/releases/8/assets{?name,label}",
            "html_url": f"https://github.com/trycua/cua/releases/tag/{body['tag_name']}",
        }
        return self.release

    def upload(self, url: str, path: Path):
        self.uploads.append((url, path.name))
        self.assets.append(
            {
                "id": len(self.assets) + 1,
                "name": path.name,
                "state": "uploaded",
                "size": path.stat().st_size,
            }
        )

    def patch(self, path: str, body: dict):
        self.patches.append((path, body))
        self.release.update(body)
        return self.release

    def delete(self, path: str):
        raise AssertionError(f"unexpected asset deletion: {path}")


@pytest.mark.parametrize(
    ("component_name", "change_path", "stage_source", "title"),
    [
        (
            "cua-driver-rs",
            "libs/cua-driver/rust/nightly-rehearsal.txt",
            "libs/cua-driver/rust/Skills/cua-driver",
            "fix(cua-driver): rehearse nightly publication",
        ),
        (
            "lume",
            "libs/lume/nightly-rehearsal.txt",
            "libs/lume/src",
            "fix(lume): rehearse nightly publication",
        ),
    ],
)
def test_nightly_transaction_rehearses_plan_stage_manifest_publish_and_retry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    component_name: str,
    change_path: str,
    stage_source: str,
    title: str,
):
    registry_path, root = copy_version_fixture(tmp_path, component_name)
    attribution_config = root / ".github/release-attribution-config.json"
    attribution_config.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / ".github/release-attribution-config.json", attribution_config)

    git(root, "init")
    git(root, "config", "user.name", "Release Rehearsal")
    git(root, "config", "user.email", "release-rehearsal@example.com")
    git(root, "add", ".")
    git(root, "commit", "-m", "chore: seed stable release fixture")
    component = component_descriptor(component_name, registry_path, root=root)
    stable_version = repository_path(root, component["versionAuthorityFile"]).read_text().strip()
    stable_tag = format_tag(component, "stable", stable_version)
    git(root, "tag", stable_tag)

    if component_name == "cua-driver-rs":
        installer = root / "libs/cua-driver/scripts/install.sh"
        installer.parent.mkdir(parents=True, exist_ok=True)
        installer.write_text("# stable installer state\n", encoding="utf-8")
        git(root, "add", str(installer.relative_to(root)))
        git(
            root,
            "commit",
            "-m",
            f"chore(cua-driver): advance published installer version to {stable_version} [skip ci]",
        )

    changed = root / change_path
    changed.parent.mkdir(parents=True, exist_ok=True)
    changed.write_text("nightly transaction change\n", encoding="utf-8")
    git(root, "add", change_path)
    git(root, "commit", "-m", f"{title} (#42)")
    source_sha = git(root, "rev-parse", "HEAD")

    monkeypatch.setattr(release_channels, "ROOT", root)
    releases_path = root / "published-releases.json"
    releases_path.write_text("[]\n", encoding="utf-8")
    plan_path = root / "release-plan.json"
    assert (
        release_channels.main(
            [
                "--registry",
                str(registry_path),
                "plan",
                "--component",
                component_name,
                "--source-sha",
                source_sha,
                "--date",
                "20260812",
                "--run",
                "42",
                "--releases",
                str(releases_path),
                "--output",
                str(plan_path),
            ]
        )
        == 0
    )
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    assert plan["shouldBuild"] is True
    assert plan["attributionBaseTag"] == stable_tag

    stage_destination = f"release-stage/{component_name}"
    assert (
        release_channels.main(
            [
                "--registry",
                str(registry_path),
                "stage-versioned-tree",
                "--component",
                component_name,
                "--version",
                plan["version"],
                "--source",
                stage_source,
                "--destination",
                stage_destination,
            ]
        )
        == 0
    )
    assert repository_path(root, component["versionAuthorityFile"]).read_text().strip() == (
        stable_version
    )
    staged_text = "\n".join(
        path.read_text(encoding="utf-8")
        for path in (root / stage_destination).rglob("*")
        if path.is_file()
    )
    assert plan["version"] in staged_text
    assets = root / "release-upload"
    assets.mkdir()
    (assets / f"{component_name}.tar.gz").write_bytes(b"nightly artifact")
    monkeypatch.setattr(
        release_channels.release_attribution,
        "GitHubClient",
        lambda *_args: RehearsalGitHub(source_sha, title),
    )
    manifest_path = root / "release-manifest.json"
    assert (
        release_channels.main(
            [
                "--registry",
                str(registry_path),
                "manifest",
                "--component",
                component_name,
                "--version",
                plan["version"],
                "--tag",
                plan["tag"],
                "--source-sha",
                source_sha,
                "--previous-tag",
                plan["attributionBaseTag"],
                "--asset-dir",
                str(assets),
                "--repository",
                "trycua/cua",
                "--output",
                str(manifest_path),
            ]
        )
        == 0
    )
    body_path = root / "release-body.md"
    assert (
        release_channels.main(
            [
                "render-nightly",
                "--manifest",
                str(manifest_path),
                "--body",
                str(body_path),
            ]
        )
        == 0
    )
    shutil.copy2(manifest_path, assets / "release-manifest.json")

    api = RehearsalReleaseApi(source_sha)
    monkeypatch.setattr(github_release, "GitHubApi", lambda *_args, **_kwargs: api)
    publish_args = [
        "--repository",
        "trycua/cua",
        "--tag",
        plan["tag"],
        "--sha",
        source_sha,
        "--body",
        str(body_path),
        "--asset-dir",
        str(assets),
        "--channel",
        "nightly",
        "--create-if-missing",
        "--prerelease",
    ]
    assert github_release.main(publish_args) == 0
    first_counts = (len(api.posts), len(api.uploads), len(api.patches))
    assert github_release.main(publish_args) == 0

    assert api.release["draft"] is False
    assert api.release["prerelease"] is True
    assert first_counts == (len(api.posts), len(api.uploads), len(api.patches))
