"""The thin cua-driver wheel: downloads the executable of its own release,
checks the SHA-256 the wheel carries, and installs or caches it; overrides work
offline. On macOS the signed app lands at the same path as the install script."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import os
import tarfile
import zipfile
from pathlib import Path

import pytest

SRC = Path(__file__).resolve().parents[1] / "src" / "cua_driver" / "fetch.py"
VERSION = "9.9.9"


@pytest.fixture()
def fetch(monkeypatch, tmp_path):
    """The source fetch module (not an installed wheel), with a private cache and app dir."""
    spec = importlib.util.spec_from_file_location("cua_driver_fetch_src", SRC)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    monkeypatch.setenv(mod.CACHE_ENV, str(tmp_path / "cache"))
    for env in (mod.BINARY_ENV, mod.BASE_ENV):
        monkeypatch.delenv(env, raising=False)
    monkeypatch.setattr(mod, "HERE", tmp_path / "pkg")
    apps = tmp_path / "Applications"
    apps.mkdir()
    monkeypatch.setattr(mod, "APP_PATH", apps / "CuaDriver.app")
    monkeypatch.setattr(mod, "APP_EXE", apps / "CuaDriver.app" / "Contents" / "MacOS" / "cua-driver")
    return mod


def add_tar(path: Path, members: dict[str, bytes]) -> None:
    with tarfile.open(path, "w:gz") as t:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o755
            t.addfile(info, io.BytesIO(data))


def add_zip(path: Path, members: dict[str, bytes]) -> None:
    with zipfile.ZipFile(path, "w") as z:
        for name, data in members.items():
            z.writestr(name, data)


def manifest_for(fetch, path: Path, base: str) -> dict:
    data = path.read_bytes()
    return {
        "version": VERSION,
        "base_url": base,
        "assets": {fetch.platform_key(): {"name": path.name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}},
    }


def served_binary(fetch, tmp_path, members=None):
    """A mirror holding the `-binary` archive of this platform (Linux, Windows)."""
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    members = members or {fetch.EXE: b"#!/bin/sh\necho cua-driver 9.9.9\n", "cua-cursor-theme": b"theme"}
    if fetch.platform_key().startswith("windows"):
        path = mirror / f"cua-driver-rs-{VERSION}-{fetch.platform_key()}-binary.zip"
        add_zip(path, members)
    else:
        path = mirror / f"cua-driver-rs-{VERSION}-{fetch.platform_key()}-binary.tar.gz"
        add_tar(path, members)
    return path, manifest_for(fetch, path, mirror.as_uri() + "/")


def served_app(fetch, tmp_path, members=None):
    """A mirror holding the macOS archive: a top-level dir with CuaDriver.app inside."""
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    top = f"cua-driver-rs-{VERSION}-darwin-universal"
    members = members or {
        f"{top}/CuaDriver.app/Contents/MacOS/cua-driver": b"#!/bin/sh\necho cua-driver 9.9.9\n",
        f"{top}/CuaDriver.app/Contents/Info.plist": b"plist",
        f"{top}/cua-driver": b"bare cli, not installed",
        f"{top}/libcua_driver_sdk.dylib": b"lib",
    }
    path = mirror / f"{top}.tar.gz"
    add_tar(path, members)
    return path, manifest_for_key(path, mirror.as_uri() + "/")


def manifest_for_key(path: Path, base: str) -> dict:
    data = path.read_bytes()
    return {
        "version": VERSION,
        "base_url": base,
        "assets": {"darwin-universal": {"name": path.name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}},
    }


@pytest.fixture()
def on_linux(fetch, monkeypatch):
    """Exercise the cache path whatever the host OS is."""
    monkeypatch.setattr(fetch.sys, "platform", "linux")
    monkeypatch.setattr(fetch.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(fetch.platform, "libc_ver", lambda: ("glibc", "2.35"))
    return fetch


@pytest.fixture()
def on_macos(fetch, monkeypatch):
    monkeypatch.setattr(fetch.sys, "platform", "darwin")
    monkeypatch.setattr(fetch.platform, "machine", lambda: "arm64")
    monkeypatch.setattr(fetch, "_verify_app_signature", lambda app: None)
    return fetch


# --- Linux / Windows cache ---------------------------------------------------


def test_first_use_downloads_verifies_and_caches(on_linux, tmp_path, capsys):
    fetch = on_linux
    path, manifest = served_binary(fetch, tmp_path)
    exe = fetch.fetch(manifest)
    assert exe.read_bytes().startswith(b"#!/bin/sh")
    assert (exe.parent / "cua-cursor-theme").read_bytes() == b"theme"
    assert os.access(exe, os.X_OK)
    assert "downloading Cua Driver 9.9.9" in capsys.readouterr().err
    # Second use: no download (the archive can even be gone).
    path.unlink()
    assert fetch.fetch(manifest) == exe


def test_the_sdk_library_is_not_unpacked_twice(on_linux, tmp_path):
    fetch = on_linux
    _, manifest = served_binary(
        fetch,
        tmp_path,
        {fetch.EXE: b"cli", "libcua_driver_sdk.so": b"lib", "cua_driver_node_runtime.node": b"n", "cua_driver_abi.h": b"h"},
    )
    exe = fetch.fetch(manifest, quiet=True)
    assert sorted(p.name for p in exe.parent.iterdir()) == sorted([fetch.EXE, fetch.VERIFIED])


def test_nested_files_such_as_the_wayland_helper_are_kept(on_linux, tmp_path):
    fetch = on_linux
    _, manifest = served_binary(fetch, tmp_path, {fetch.EXE: b"cli", "wayland-helper/install.sh": b"#!/bin/sh\n"})
    exe = fetch.fetch(manifest, quiet=True)
    assert (exe.parent / "wayland-helper" / "install.sh").is_file()


def test_a_tampered_archive_is_refused(on_linux, tmp_path):
    fetch = on_linux
    path, manifest = served_binary(fetch, tmp_path)
    data = bytearray(path.read_bytes())
    data[-1] ^= 0xFF
    path.write_bytes(bytes(data))
    with pytest.raises(fetch.FetchError, match="does not match"):
        fetch.fetch(manifest)
    assert not (Path(os.environ[fetch.CACHE_ENV]) / VERSION / fetch.platform_key() / fetch.EXE).exists()


def test_an_oversized_download_is_refused(on_linux, tmp_path):
    fetch = on_linux
    _, manifest = served_binary(fetch, tmp_path)
    manifest["assets"][fetch.platform_key()]["size"] = 10
    with pytest.raises(fetch.FetchError, match="larger than"):
        fetch.fetch(manifest)


@pytest.mark.parametrize("name", ["../evil", "/abs/evil", "a/../../evil"])
def test_path_traversal_is_refused(on_linux, tmp_path, name):
    fetch = on_linux
    _, manifest = served_binary(fetch, tmp_path, {name: b"x", fetch.EXE: b"x"})
    with pytest.raises(fetch.FetchError, match="unexpected entry"):
        fetch.fetch(manifest)
    assert not (tmp_path / "cache" / VERSION / "evil").exists()
    assert not (tmp_path / "evil").exists()


def test_a_link_in_the_archive_is_refused(on_linux, tmp_path):
    fetch = on_linux
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    path = mirror / f"cua-driver-rs-{VERSION}-{fetch.platform_key()}-binary.tar.gz"
    with tarfile.open(path, "w:gz") as t:
        link = tarfile.TarInfo("cua-driver")
        link.type = tarfile.SYMTYPE
        link.linkname = "/etc/passwd"
        t.addfile(link)
    manifest = manifest_for(fetch, path, mirror.as_uri() + "/")
    with pytest.raises(fetch.FetchError, match="unexpected entry"):
        fetch.fetch(manifest)


def test_an_archive_without_the_executable_is_refused(on_linux, tmp_path):
    fetch = on_linux
    _, manifest = served_binary(fetch, tmp_path, {"readme": b"x"})
    with pytest.raises(fetch.FetchError, match="has no"):
        fetch.fetch(manifest)


def test_a_mirror_overrides_the_release_url(on_linux, tmp_path, monkeypatch):
    fetch = on_linux
    path, manifest = served_binary(fetch, tmp_path)
    manifest["base_url"] = "https://invalid.example/never/"
    monkeypatch.setenv(fetch.BASE_ENV, path.parent.as_uri())
    assert fetch.fetch(manifest, quiet=True).is_file()


def test_unreachable_download_is_a_fetch_error(on_linux, tmp_path):
    fetch = on_linux
    _, manifest = served_binary(fetch, tmp_path)
    manifest["base_url"] = (tmp_path / "nowhere").as_uri() + "/"
    with pytest.raises(fetch.FetchError, match="cannot download"):
        fetch.fetch(manifest, quiet=True)


def test_a_platform_the_release_does_not_build_is_a_fetch_error(on_linux, tmp_path):
    fetch = on_linux
    _, manifest = served_binary(fetch, tmp_path)
    manifest["assets"] = {"linux-riscv": {}}
    with pytest.raises(fetch.FetchError, match="has no build"):
        fetch.fetch(manifest)


# --- macOS app install ---------------------------------------------------------


def test_macos_installs_the_signed_app_at_applications(on_macos, tmp_path):
    fetch = on_macos
    _, manifest = served_app(fetch, tmp_path)
    exe = fetch.fetch(manifest, quiet=True)
    assert exe == fetch.APP_EXE
    assert exe.read_bytes().startswith(b"#!/bin/sh")
    assert os.access(exe, os.X_OK)
    assert (fetch.APP_PATH / "Contents" / "Info.plist").is_file()
    # Only the app is installed: not the bare CLI or the library next to it.
    assert sorted(p.name for p in fetch.APP_PATH.parent.iterdir()) == ["CuaDriver.app"]


def test_macos_install_checks_the_signature_before_moving_the_app(on_macos, tmp_path, monkeypatch):
    fetch = on_macos
    _, manifest = served_app(fetch, tmp_path)

    def refuse(app):
        assert (app / "Contents" / "MacOS" / "cua-driver").is_file()
        raise fetch.FetchError("not signed by Cua AI, Inc.")

    monkeypatch.setattr(fetch, "_verify_app_signature", refuse)
    with pytest.raises(fetch.FetchError, match="not signed"):
        fetch.fetch(manifest, quiet=True)
    assert not fetch.APP_PATH.exists()
    assert list(fetch.APP_PATH.parent.iterdir()) == []  # staging is cleaned up


def test_macos_signature_requirement_pins_the_cua_team_and_bundle_id(fetch):
    assert 'identifier "com.trycua.driver"' in fetch.APP_REQUIREMENT
    assert 'certificate leaf[subject.OU] = "YCK386LBJ7"' in fetch.APP_REQUIREMENT
    assert fetch.APP_REQUIREMENT.startswith("=anchor apple generic")


def test_macos_a_tampered_archive_never_reaches_applications(on_macos, tmp_path):
    fetch = on_macos
    path, manifest = served_app(fetch, tmp_path)
    data = bytearray(path.read_bytes())
    data[-1] ^= 0xFF
    path.write_bytes(bytes(data))
    with pytest.raises(fetch.FetchError, match="does not match"):
        fetch.fetch(manifest, quiet=True)
    assert list(fetch.APP_PATH.parent.iterdir()) == []


def test_macos_an_existing_app_is_used_and_never_replaced(on_macos, tmp_path, monkeypatch, capsys):
    fetch = on_macos
    fetch.APP_EXE.parent.mkdir(parents=True)
    fetch.APP_EXE.write_bytes(b"installed by the user")
    monkeypatch.setattr(fetch, "_download", lambda *a, **k: pytest.fail("downloaded"))
    _, manifest = served_app(fetch, tmp_path)
    assert fetch.fetch(manifest) == fetch.APP_EXE
    assert fetch.APP_EXE.read_bytes() == b"installed by the user"
    assert capsys.readouterr().err == ""


def test_macos_an_older_app_is_kept_with_a_hint(on_macos, tmp_path, monkeypatch, capsys):
    fetch = on_macos
    fetch.APP_EXE.parent.mkdir(parents=True)
    fetch.APP_EXE.write_bytes(b"old")
    monkeypatch.setattr(fetch, "installed_app_version", lambda: "9.9.0")
    _, manifest = served_app(fetch, tmp_path)
    assert fetch.fetch(manifest) == fetch.APP_EXE
    assert "cua-driver update --apply" in capsys.readouterr().err


def test_macos_an_unwritable_applications_folder_points_at_the_installer(on_macos, tmp_path, monkeypatch):
    fetch = on_macos
    _, manifest = served_app(fetch, tmp_path)
    monkeypatch.setattr(fetch.os, "access", lambda *a, **k: False)
    with pytest.raises(fetch.FetchError, match="install.sh"):
        fetch.fetch(manifest, quiet=True)


def test_macos_an_archive_without_the_app_is_refused(on_macos, tmp_path):
    fetch = on_macos
    _, manifest = served_app(fetch, tmp_path, {"top/cua-driver": b"x"})
    with pytest.raises(fetch.FetchError, match="no CuaDriver.app"):
        fetch.fetch(manifest, quiet=True)


def test_macos_path_traversal_in_the_app_archive_is_refused(on_macos, tmp_path):
    fetch = on_macos
    _, manifest = served_app(fetch, tmp_path, {"top/CuaDriver.app/../../evil": b"x"})
    with pytest.raises(fetch.FetchError, match="unexpected entry"):
        fetch.fetch(manifest, quiet=True)


# --- which executable runs -------------------------------------------------------


def test_an_explicit_binary_wins_and_never_downloads(fetch, monkeypatch, tmp_path):
    monkeypatch.setenv(fetch.BINARY_ENV, str(tmp_path / "my-driver"))
    monkeypatch.setattr(fetch, "fetch", lambda *a, **k: pytest.fail("downloaded"))
    assert fetch.binary() == tmp_path / "my-driver"


def test_a_bundled_binary_wins_over_the_manifest(fetch, tmp_path, monkeypatch):
    (tmp_path / "pkg" / "bin").mkdir(parents=True)
    (tmp_path / "pkg" / "bin" / fetch.EXE).write_bytes(b"x")
    (tmp_path / "pkg" / "_release.json").write_text("{}")
    monkeypatch.setattr(fetch, "fetch", lambda *a, **k: pytest.fail("downloaded"))
    assert fetch.binary() == tmp_path / "pkg" / "bin" / fetch.EXE


def test_without_a_manifest_or_binary_nothing_is_downloaded(fetch, tmp_path, monkeypatch):
    monkeypatch.setattr(fetch, "fetch", lambda *a, **k: pytest.fail("downloaded"))
    assert fetch.binary() == tmp_path / "pkg" / "bin" / fetch.EXE


def test_the_cli_command_reports_a_failed_fetch(fetch, tmp_path, capsys):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "_release.json").write_text('{"version": "9.9.9", "base_url": "file:///nowhere/", "assets": {}}')
    assert fetch.main(["fetch"]) == 1
    assert "cua-driver:" in capsys.readouterr().err
    assert fetch.main(["nope"]) == 2


def test_wrapper_fetches_when_the_wheel_is_thin(monkeypatch, tmp_path):
    spec = importlib.util.spec_from_file_location("cua_driver_wrapper_thin", SRC.parent / "wrapper.py")
    wrapper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(wrapper)
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "_release.json").write_text("{}")
    monkeypatch.setattr(wrapper, "__file__", str(pkg / "wrapper.py"))
    monkeypatch.delenv("CUA_DRIVER_BINARY", raising=False)
    exe = tmp_path / "fetched" / "cua-driver"
    exe.parent.mkdir()
    exe.write_bytes(b"x")
    exe.chmod(0o755)

    class Fake:
        class FetchError(RuntimeError):
            pass

        @staticmethod
        def binary():
            return exe

    monkeypatch.setattr(wrapper, "_fetch_module", lambda: Fake)
    assert wrapper.get_binary_path() == exe

    def boom():
        raise Fake.FetchError("offline")

    Fake.binary = staticmethod(boom)
    with pytest.raises(FileNotFoundError, match="offline"):
        wrapper.get_binary_path()
