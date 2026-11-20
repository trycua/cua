"""The thin `cua-cli` wheel: downloads the CLI of its own release, checks the
SHA-256 the wheel carries, and caches it; overrides work offline."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import os
import tarfile
import zipfile
from pathlib import Path

import pytest

SRC = Path(__file__).resolve().parents[1] / "cli" / "src" / "cua_cli" / "__init__.py"


@pytest.fixture()
def cli(monkeypatch, tmp_path):
    """The source cua_cli module (not an installed wheel), with a private cache."""
    spec = importlib.util.spec_from_file_location("cua_cli_src", SRC)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    monkeypatch.setenv(mod.CACHE_ENV, str(tmp_path / "cache"))
    for env in (mod.BINARY_ENV, mod.BASE_ENV):
        monkeypatch.delenv(env, raising=False)
    monkeypatch.setattr(mod, "HERE", tmp_path / "pkg")
    return mod


def archive(dest: Path, key: str, members: dict[str, bytes]) -> Path:
    if key.startswith("windows"):
        path = dest / f"cua-cli-9.9.9-{key}.zip"
        with zipfile.ZipFile(path, "w") as z:
            for name, data in members.items():
                z.writestr(name, data)
    else:
        path = dest / f"cua-cli-9.9.9-{key}.tar.gz"
        with tarfile.open(path, "w:gz") as t:
            for name, data in members.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                t.addfile(info, io.BytesIO(data))
    return path


def manifest_for(cli, path: Path, base: str) -> dict:
    data = path.read_bytes()
    return {
        "version": "9.9.9",
        "base_url": base,
        "assets": {cli.platform_key(): {"name": path.name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}},
    }


def served(cli, tmp_path, members=None):
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    path = archive(mirror, cli.platform_key(), members or {cli.EXE: b"#!/bin/sh\necho cua 9.9.9\n", "cua-vz": b"vz"})
    return path, manifest_for(cli, path, mirror.as_uri() + "/")


def test_first_use_downloads_verifies_and_caches(cli, tmp_path, capsys):
    path, manifest = served(cli, tmp_path)
    exe = cli.fetch(manifest)
    assert exe.read_bytes().startswith(b"#!/bin/sh")
    assert (exe.parent / "cua-vz").read_bytes() == b"vz"
    if os.name != "nt":
        assert os.access(exe, os.X_OK)
    assert "downloading the cua CLI 9.9.9" in capsys.readouterr().err
    # Second use: no download (the archive can even be gone).
    path.unlink()
    assert cli.fetch(manifest) == exe


def test_a_tampered_archive_is_refused(cli, tmp_path):
    path, manifest = served(cli, tmp_path)
    data = bytearray(path.read_bytes())
    data[-1] ^= 0xFF
    path.write_bytes(bytes(data))
    with pytest.raises(cli.FetchError, match="does not match"):
        cli.fetch(manifest)
    assert not (Path(os.environ[cli.CACHE_ENV]) / "9.9.9" / cli.platform_key() / cli.EXE).exists()


def test_path_entries_are_refused(cli, tmp_path):
    _, manifest = served(cli, tmp_path, {"../evil": b"x", cli.EXE: b"x"})
    with pytest.raises(cli.FetchError, match="unexpected entry"):
        cli.fetch(manifest)
    assert not (tmp_path / "cache" / "9.9.9" / "evil").exists()


def test_a_mirror_overrides_the_release_url(cli, tmp_path, monkeypatch):
    path, manifest = served(cli, tmp_path)
    manifest["base_url"] = "https://invalid.example/never/"
    monkeypatch.setenv(cli.BASE_ENV, path.parent.as_uri())
    assert cli.fetch(manifest, quiet=True).is_file()


def test_an_explicit_binary_wins_and_never_downloads(cli, monkeypatch, tmp_path):
    monkeypatch.setenv(cli.BINARY_ENV, str(tmp_path / "my-cua"))
    monkeypatch.setattr(cli, "fetch", lambda *a, **k: pytest.fail("downloaded"))
    assert cli.binary() == tmp_path / "my-cua"


def test_a_bundled_binary_wins_over_the_manifest(cli, tmp_path, monkeypatch):
    (tmp_path / "pkg" / "bin").mkdir(parents=True)
    (tmp_path / "pkg" / "bin" / cli.EXE).write_bytes(b"x")
    (tmp_path / "pkg" / "_release.json").write_text("{}")
    monkeypatch.setattr(cli, "fetch", lambda *a, **k: pytest.fail("downloaded"))
    assert cli.binary() == tmp_path / "pkg" / "bin" / cli.EXE


def test_unreachable_download_is_a_fetch_error(cli, tmp_path):
    _, manifest = served(cli, tmp_path)
    manifest["base_url"] = (tmp_path / "nowhere").as_uri() + "/"
    with pytest.raises(cli.FetchError, match="cannot download"):
        cli.fetch(manifest, quiet=True)


def test_platform_keys_match_the_release_archives(cli):
    import sys

    sys.path.insert(0, str(SRC.parents[2]))
    try:
        import build_thin_wheel
    finally:
        sys.path.pop(0)
    assert cli.platform_key() in build_thin_wheel.PLATFORMS
