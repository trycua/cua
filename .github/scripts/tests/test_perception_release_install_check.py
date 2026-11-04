"""Tests for the Cua Perception release install check."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import sys
import textwrap
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "perception_release_install_check",
    ROOT / ".github/scripts/perception_release_install_check.py",
)
check = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(check)

SOURCE_SHA = "a" * 40
TARGET = "x86_64-unknown-linux-gnu"
VERSION = "0.2.1"

FAKE_DRIVER = textwrap.dedent(
    """\
    #!{python}
    import json, os, pathlib, sys

    home = pathlib.Path(os.environ["CUA_DRIVER_RS_HOME"])
    mode = os.environ.get("FAKE_MODE", "")
    state = home / "installed.json"
    args = sys.argv[1:]
    log = home.parent / (home.name + "-calls.jsonl")
    with log.open("a") as handle:
        handle.write(json.dumps(args) + "\\n")

    def emit(value, code=0):
        print(json.dumps(value))
        raise SystemExit(code)

    def source_trust():
        if "--catalog" in args:
            if mode == "signed-reports-developer":
                return "developer-unsigned-local", "developer-local-not-publisher-evidence"
            return "publisher-verified", "production-publisher-verified"
        assert "--allow-unsigned-local" in args
        return "developer-unsigned-local", "developer-local-not-publisher-evidence"

    if args == ["--version"]:
        print("cua-driver 0.29.1")
        raise SystemExit(0)
    if args[:2] == ["extension", "status"]:
        if state.exists():
            record = json.loads(state.read_text())
            emit({{"id": "cua-perception", "installed": True, "healthy": mode != "unhealthy",
                  "active_version": record["version"], "trust": record["trust"],
                  "evidence_class": record["evidence"], "detail": "ok"}})
        emit({{"id": "cua-perception", "installed": False, "healthy": True, "detail": "not installed"}})
    if args[:2] == ["perception", "parse"]:
        if not state.exists():
            if mode == "parse-without-extension-succeeds":
                emit({{"schema": "cua.visual_regions_v1"}})
            emit({{"ok": False, "error": {{"code": "not_installed", "message": "absent"}}}}, 1)
        image = pathlib.Path(args[args.index("--image") + 1])
        import hashlib
        digest = hashlib.sha256(image.read_bytes()).hexdigest()
        regions = [] if mode == "no-regions" else [
            {{"id": "text-1", "kind": "text", "confidence": 0.65,
              "bounds": {{"x": 7, "y": 17, "width": 5, "height": 6}}}}
        ]
        emit({{"schema": "cua.visual_regions_v1",
              "parser": {{"extension_id": "cua-perception",
                         "extension_version": json.loads(state.read_text())["version"]}},
              "local_input": {{"action_eligible": False, "action_authority": "none", "sha256": digest}},
              "regions": regions, "timing": {{"duration_ms": 12}}}})
    if args[:2] == ["extension", "inspect"]:
        trust, evidence = source_trust()
        payload = {{}}
        if "--catalog" in args:
            payload = json.loads(pathlib.Path(args[args.index("--catalog") + 1]).read_text())["payload"]
        emit({{"id": "cua-perception", "version": os.environ["FAKE_VERSION"],
              "target": os.environ["FAKE_TARGET"], "installed": False,
              "mutation_performed": mode == "inspect-mutates", "ran": False,
              "corresponding_source_revision": os.environ["FAKE_SOURCE_SHA"],
              "models": [{{"path": "models/a.onnx"}}], "license_notices": [{{"component": "a"}}],
              "corresponding_source": {{"uri": "source/cua-perception-source.tar.gz"}},
              "trust": trust, "evidence_class": evidence,
              "publisher_signature_verified": "--catalog" in args,
              "publisher_id": "cua" if "--catalog" in args else None,
              "publisher_key_id": payload.get("key_id"),
              "catalog_version": payload.get("catalog_version"),
              "archive_sha256": payload.get("archive_sha256")}})
    if args[:2] == ["extension", "install"]:
        trust, evidence = source_trust()
        state.write_text(json.dumps({{"version": os.environ["FAKE_VERSION"], "trust": trust,
                                     "evidence": evidence}}))
        print("Installed")
        raise SystemExit(0)
    if args[:2] == ["extension", "remove"]:
        if mode != "remove-noop":
            state.unlink()
        print("Removed")
        raise SystemExit(0)
    print("unexpected arguments: " + " ".join(args), file=sys.stderr)
    raise SystemExit(2)
    """
)


@pytest.fixture
def layout(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> dict[str, Path]:
    driver = tmp_path / "bin" / "cua-driver"
    driver.parent.mkdir()
    driver.write_text(FAKE_DRIVER.format(python=sys.executable), encoding="utf-8")
    driver.chmod(driver.stat().st_mode | stat.S_IEXEC)
    release = tmp_path / "release"
    release.mkdir()
    archive = release / f"cua-perception-{VERSION}-{TARGET}.tar.gz"
    archive.write_bytes(b"archive bytes")
    catalog = release / "signed-catalog.json"
    catalog.write_text(json.dumps({
        "payload": {
            "version": VERSION,
            "target": TARGET,
            "corresponding_source_revision": SOURCE_SHA,
            "archive": archive.name,
            "archive_sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
            "key_id": "cua-extension-ed25519-2026-09",
            "catalog_version": 2609260612,
        },
        "signature_algorithm": "ed25519",
        "signature": "AA==",
    }), encoding="utf-8")
    image = tmp_path / "fixture.png"
    image.write_bytes(b"\x89PNG\r\n\x1a\nfixture")
    monkeypatch.setenv("FAKE_VERSION", VERSION)
    monkeypatch.setenv("FAKE_TARGET", TARGET)
    monkeypatch.setenv("FAKE_SOURCE_SHA", SOURCE_SHA)
    monkeypatch.delenv("FAKE_MODE", raising=False)
    return {"driver": driver, "catalog": catalog, "archive": archive, "image": image, "root": tmp_path}


def arguments(layout: dict[str, Path], *, signed: bool = True, version: str = VERSION) -> list[str]:
    source = ["--catalog", str(layout["catalog"])] if signed else [
        "--unsigned-archive", str(layout["archive"])
    ]
    return [
        "--driver", str(layout["driver"]),
        *source,
        "--version", version,
        "--target", TARGET,
        "--source-sha", SOURCE_SHA,
        "--image", str(layout["image"]),
        "--home", str(layout["root"] / "home"),
        "--evidence", str(layout["root"] / "evidence.json"),
    ]


def test_signed_catalog_passes_the_full_lifecycle(layout: dict[str, Path]) -> None:
    assert check.main(arguments(layout)) == 0
    evidence = json.loads((layout["root"] / "evidence.json").read_text())
    assert evidence["trust"] == "publisher-verified"
    assert evidence["evidence_class"] == "production-publisher-verified"
    assert evidence["catalog_version"] == 2609260612
    assert evidence["driver_version"] == "cua-driver 0.29.1"
    assert evidence["checks"] == [
        "absent-not-installed",
        "inspect-without-mutation",
        "install",
        "status-self-test-healthy",
        "real-parse",
        "remove",
    ]
    calls = [
        json.loads(line)
        for line in (layout["root"] / "home-calls.jsonl").read_text().splitlines()
    ]
    commands = [call[:2] for call in calls]
    assert commands.index(["extension", "inspect"]) < commands.index(["extension", "install"])
    assert ["extension", "status", "cua-perception", "--self-test", "--json"] in calls
    assert all("--allow-unsigned-local" not in call for call in calls)


def test_unsigned_dry_run_reports_developer_trust_only(layout: dict[str, Path]) -> None:
    assert check.main(arguments(layout, signed=False)) == 0
    evidence = json.loads((layout["root"] / "evidence.json").read_text())
    assert evidence["trust"] == "developer-unsigned-local"
    assert evidence["evidence_class"] == "developer-local-not-publisher-evidence"


@pytest.mark.parametrize(
    "mode",
    [
        "signed-reports-developer",
        "parse-without-extension-succeeds",
        "inspect-mutates",
        "unhealthy",
        "no-regions",
        "remove-noop",
    ],
)
def test_contract_violations_fail_closed(
    layout: dict[str, Path], monkeypatch: pytest.MonkeyPatch, mode: str,
) -> None:
    monkeypatch.setenv("FAKE_MODE", mode)
    assert check.main(arguments(layout)) == 1
    assert not (layout["root"] / "evidence.json").exists()


def test_catalog_must_match_the_release_identity(layout: dict[str, Path]) -> None:
    assert check.main(arguments(layout, version="0.2.2")) == 1


def test_catalog_archive_digest_is_checked_before_the_driver_runs(layout: dict[str, Path]) -> None:
    layout["archive"].write_bytes(b"tampered")
    assert check.main(arguments(layout)) == 1
    assert not (layout["root"] / "home-calls.jsonl").exists()


def test_refuses_a_reused_driver_home(layout: dict[str, Path]) -> None:
    (layout["root"] / "home").mkdir()
    assert check.main(arguments(layout)) == 1


def test_relative_driver_path_never_resolves_through_path(
    layout: dict[str, Path], monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.chdir(layout["driver"].parent)
    monkeypatch.setenv("PATH", "/nonexistent")
    args = arguments(layout)
    args[args.index("--driver") + 1] = "./cua-driver"
    assert check.main(args) == 0
    assert os.path.isabs(str(check.Driver(Path("./cua-driver"), layout["root"] / "x").executable))
