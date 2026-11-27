"""Icon extraction in creator submit_result: agent-supplied paths never reach a shell unquoted.

Fixes trycua/cua#8523 (reported by soarkit).
"""

from __future__ import annotations

import asyncio
import base64
import shlex
import struct
import subprocess
import zlib
from pathlib import Path
from types import SimpleNamespace

import pytest

pytest.importorskip("claude_agent_sdk")

from cua_sandbox_apps.pipeline import creator_agent  # noqa: E402

NASTY_PATHS = [
    "icon.png",
    "with space.png",
    "semi; touch PWNED.png",
    "pipe|touch PWNED.png",
    "dollar$(touch PWNED).png",
    "back`touch PWNED`tick.png",
    "single'quote.png",
    'double"quote.png',
    "-leading-dash.png",
    "amp&&touch PWNED.png",
]


def _png(width: int, height: int) -> bytes:
    def chunk(kind: bytes, body: bytes) -> bytes:
        crc = zlib.crc32(kind + body) & 0xFFFFFFFF
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    raw = b"".join(b"\x00" + b"\x00\x00\x00" * width for _ in range(height))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


class RecordingShell:
    """Runs commands with bash in a scratch directory, like a real guest."""

    def __init__(self, cwd: Path):
        self.cwd = cwd
        self.commands: list[str] = []

    async def run(self, command: str, timeout: int = 120):
        self.commands.append(command)
        p = subprocess.run(
            ["bash", "-c", command], cwd=self.cwd, capture_output=True, text=True, timeout=timeout
        )
        return SimpleNamespace(stdout=p.stdout, stderr=p.stderr, returncode=p.returncode)


class FileApi:
    def __init__(self, cwd: Path):
        self.cwd = cwd
        self.reads: list[str] = []

    async def read_bytes(self, path: str) -> bytes:
        self.reads.append(path)
        return (self.cwd / path).read_bytes()


class NoFileApi:
    async def read_bytes(self, path: str) -> bytes:
        raise NotImplementedError("no file API on this image")


def _sandbox(tmp_path: Path, *, file_api: bool):
    guest = tmp_path / "guest"
    guest.mkdir()
    shell = RecordingShell(guest)
    files = FileApi(guest) if file_api else NoFileApi()
    return guest, SimpleNamespace(shell=shell, files=files)


@pytest.mark.parametrize("file_api", [True, False], ids=["file-api", "shell-fallback"])
@pytest.mark.parametrize("name", NASTY_PATHS)
def test_nasty_icon_path_is_one_literal_argument(tmp_path, file_api, name):
    guest, sb = _sandbox(tmp_path, file_api=file_api)
    (guest / name).write_bytes(_png(64, 64))
    out = tmp_path / "out"
    out.mkdir()

    saved = asyncio.run(creator_agent._extract_icon(sb, name, out))

    assert saved is not None and saved.read_bytes() == _png(64, 64)
    assert saved.name == "install-icon-64x64.png"
    assert not (guest / "PWNED").exists()
    if file_api:
        assert sb.shell.commands == []
        assert sb.files.reads == [name]
    else:
        assert sb.shell.commands == [f"base64 < {shlex.quote(name)} 2>/dev/null"]
        assert shlex.split(sb.shell.commands[0]) == ["base64", "<", name, "2>/dev/null"]


def test_injection_in_missing_file_runs_nothing(tmp_path):
    guest, sb = _sandbox(tmp_path, file_api=False)
    out = tmp_path / "out"
    out.mkdir()

    with pytest.raises(NotImplementedError):
        asyncio.run(creator_agent._extract_icon(sb, "x; touch PWNED", out))

    assert not (guest / "PWNED").exists()


def test_normal_path_uses_header_width_then_path_hint(tmp_path):
    guest, sb = _sandbox(tmp_path, file_api=True)
    (guest / "usr/share/icons/hicolor/256x256/apps").mkdir(parents=True)
    (guest / "usr/share/icons/hicolor/256x256/apps/app.png").write_bytes(_png(32, 32))
    (guest / "usr/share/icons/hicolor/128x128.svg").parent.mkdir(parents=True, exist_ok=True)
    (guest / "usr/share/icons/hicolor/128x128.svg").write_bytes(b"<svg/>")
    out = tmp_path / "out"
    out.mkdir()

    png = asyncio.run(
        creator_agent._extract_icon(sb, "usr/share/icons/hicolor/256x256/apps/app.png", out)
    )
    svg = asyncio.run(creator_agent._extract_icon(sb, "usr/share/icons/hicolor/128x128.svg", out))

    assert png.name == "install-icon-32x32.png"  # real PNG width wins over the path
    assert svg.name == "install-icon-128x128.svg"  # no header: falls back to the path


def test_unsafe_extension_and_non_string_paths_are_rejected(tmp_path):
    guest, sb = _sandbox(tmp_path, file_api=True)
    (guest / "a.p ng;x").write_bytes(b"data")
    out = tmp_path / "out"
    out.mkdir()

    saved = asyncio.run(creator_agent._extract_icon(sb, "a.p ng;x", out))
    assert saved is not None and saved.suffix == ".png"
    assert asyncio.run(creator_agent._extract_icon(sb, ["x"], out)) is None
    assert asyncio.run(creator_agent._extract_icon(sb, "", out)) is None


def test_base64_decode_fallback_roundtrip_matches_bytes(tmp_path):
    guest, sb = _sandbox(tmp_path, file_api=False)
    (guest / "i.png").write_bytes(_png(16, 16))
    got = asyncio.run(creator_agent._read_sandbox_file(sb, "i.png"))
    assert got == base64.b64decode(base64.b64encode(_png(16, 16)))
