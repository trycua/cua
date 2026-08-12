from __future__ import annotations

import json
from pathlib import Path
import shutil

import pytest
from jsonschema import Draft202012Validator

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
    render_nightly_body,
)


ROOT = Path(__file__).resolve().parents[3]
REGISTRY = ROOT / ".github/releases/components.json"


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
    assert derive_nightly_version("0.19.3", "20260812", "3097") == (
        "0.19.4-nightly.20260812.3097"
    )


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


def test_first_nightly_plan_is_reproducible_and_requires_build():
    source_sha = "a" * 40
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
    assert plan["tag"] == "nightly-lume-v0.5.4-nightly.20260812.42"
    assert plan["bundleVersion"] == "0.5.4"


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


def test_nightly_manifest_allows_empty_stable_change_attribution(tmp_path: Path):
    (tmp_path / "artifact.tar.gz").write_bytes(b"nightly")
    version = "0.5.4-nightly.20260812.42"
    component = component_descriptor("lume", REGISTRY, root=ROOT)
    manifest = build_manifest(
        "lume",
        version,
        format_tag(component, "nightly", version),
        "a" * 40,
        None,
        tmp_path,
        repository="trycua/cua",
        registry_path=REGISTRY,
        root=ROOT,
    )
    assert manifest["channel"] == "nightly"
    schema = json.loads((ROOT / ".github/release-manifest.schema.json").read_text())
    Draft202012Validator(schema, format_checker=None).validate(manifest)
    assert manifest["changes"] == []
    assert manifest["assets"][0]["sha256"] == (
        "2a3b62b53ddb9f167b63d22202a360811ba78df015021f704d01ee9abad4169c"
    )
    body = render_nightly_body(manifest)
    assert "LUME_VERSION=nightly-lume-v0.5.4-nightly.20260812.42" in body
    assert "never replace stable" in body
