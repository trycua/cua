"""Regression tests for trycua/cua#8573: host-side installers must not stage
downloads or run code from predictable paths in shared temp directories."""

from __future__ import annotations

import asyncio
import hashlib
import io
import os
import shutil
import stat
import subprocess
import tempfile
import zipfile
from pathlib import Path

import pytest
from cua_sandbox.builder import pwa2apk
from cua_sandbox.runtime import qemu_installer


def _zip_bytes() -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("tool/tool.exe", b"binary")
    return buf.getvalue()


def _fake_httpx(monkeypatch, payload: bytes, staging_parent: Path, seen: list):
    """Stub httpx.Client; while streaming, record where the archive is staged."""

    class Resp:
        headers = {"content-length": str(len(payload))}

        def raise_for_status(self):
            pass

        def iter_bytes(self, chunk_size=1024):
            seen.extend(
                (p, stat.S_IMODE(p.stat().st_mode)) for p in staging_parent.glob(".download-*")
            )
            yield payload

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    class Client:
        def __init__(self, **kw):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def stream(self, method, url):
            return Resp()

    monkeypatch.setattr(qemu_installer.httpx, "Client", Client)


@pytest.fixture
def no_mktemp(monkeypatch, tmp_path):
    def _boom(*a, **kw):
        raise AssertionError("tempfile.mktemp must not be used")

    monkeypatch.setattr(tempfile, "mktemp", _boom)
    shared_tmp = tmp_path / "shared-tmp"
    shared_tmp.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", str(shared_tmp))
    return shared_tmp


def test_download_stages_in_private_dir_outside_shared_tmp(monkeypatch, tmp_path, no_mktemp):
    payload = _zip_bytes()
    dest = tmp_path / "home" / "cua-sandbox" / "tool"
    seen: list = []
    _fake_httpx(monkeypatch, payload, dest.parent, seen)

    qemu_installer._download_zip(
        "https://example.invalid/t.zip", dest, "tool", hashlib.sha256(payload).hexdigest()
    )

    assert (dest / "tool" / "tool.exe").read_bytes() == b"binary"
    # Nothing touched the shared temp dir, and staging was cleaned up.
    assert list(no_mktemp.iterdir()) == []
    assert list(dest.parent.glob(".download-*")) == []
    # While downloading, the archive sat in exactly one private staging dir.
    assert len(seen) == 1
    if os.name == "posix":
        assert seen[0][1] == 0o700


def test_download_rejects_wrong_hash_and_installs_nothing(monkeypatch, tmp_path, no_mktemp):
    payload = _zip_bytes()
    dest = tmp_path / "home" / "cua-sandbox" / "tool"
    _fake_httpx(monkeypatch, payload, dest.parent, [])

    with pytest.raises(RuntimeError, match="integrity check"):
        qemu_installer._download_zip("https://example.invalid/t.zip", dest, "tool", "0" * 64)

    assert list(dest.iterdir()) == []
    assert list(dest.parent.glob(".download-*")) == []


def test_installers_pin_sha256():
    for value in (qemu_installer.QEMU_PORTABLE_SHA256, qemu_installer.WIMLIB_SHA256):
        assert len(value) == 64 and int(value, 16) >= 0


def test_pwa2apk_only_probes_cua_home(monkeypatch, tmp_path):
    probed: list[str] = []
    real_exists = Path.exists

    def recording_exists(self, *a, **kw):
        probed.append(str(self))
        return real_exists(self, *a, **kw)

    class Stop(Exception):
        pass

    def fake_run(cmd, **kw):
        raise Stop(cmd)

    monkeypatch.setattr(Path, "exists", recording_exists)
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/node" if name == "node" else None)
    monkeypatch.setattr(subprocess, "run", fake_run)

    with pytest.raises(Stop) as exc:
        asyncio.run(pwa2apk.build_pwa2apk("https://example.invalid/manifest.json"))

    home = str(pwa2apk.cua_home())
    clone_cmd = exc.value.args[0]
    assert clone_cmd[-1] == str(Path(home) / "pwa2apk")
    assert all(p.startswith(home) for p in probed if "pwa2apk" in p), probed
