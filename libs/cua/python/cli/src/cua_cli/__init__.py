"""The Rust `cua` CLI for the `cua` Python package (run through its `cua`
console command).

The PyPI wheel is thin: it carries no binary, only `_release.json`, which
names the signed CLI archive of this exact release on GitHub Releases for
each platform with its size and SHA-256. The first run downloads the archive
for this machine, checks the hash and unpacks it into a per-version cache;
later runs start the cached binary directly.

Overrides (offline, air-gapped or pinned installs):
  CUA_CLI_BINARY         use this `cua` executable; nothing is downloaded
  CUA_CLI_DOWNLOAD_BASE  fetch the archives from this URL (a mirror, or
                         file:///dir/ holding them); the hashes still apply
  CUA_CLI_CACHE_DIR      where downloaded CLIs live

`python -m cua_cli fetch` downloads ahead of time (for Docker images and CI);
`python -m cua_cli path` prints the binary it would run.

A wheel built with the binary inside (`cua_cli/bin/`, local and CI builds)
uses that and never downloads.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import shutil
import sys
import tarfile
import tempfile
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXE = "cua.exe" if os.name == "nt" else "cua"
BINARY_ENV = "CUA_CLI_BINARY"
BASE_ENV = "CUA_CLI_DOWNLOAD_BASE"
CACHE_ENV = "CUA_CLI_CACHE_DIR"
VERIFIED = ".sha256"


class FetchError(RuntimeError):
    """The CLI could not be downloaded or verified."""


def bundled() -> Path:
    """Where a wheel built with the binary inside installs it."""
    return HERE / "bin" / EXE


def release() -> dict | None:
    """The `_release.json` manifest of a thin wheel, if any."""
    path = HERE / "_release.json"
    return json.loads(path.read_text()) if path.is_file() else None


def platform_key() -> str:
    """This machine's key in the manifest, e.g. `darwin-arm64`, `windows-x64`."""
    machine = platform.machine().lower()
    arch = {"x86_64": "x64", "amd64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(machine)
    system = {"darwin": "darwin", "linux": "linux", "win32": "windows"}.get(sys.platform)
    if arch is None or system is None:
        raise FetchError(f"no cua CLI build for {sys.platform}/{machine}")
    if system == "linux" and platform.libc_ver()[0] not in ("glibc", ""):
        raise FetchError("the cua CLI needs glibc 2.31 or newer (musl is not supported)")
    return f"{system}-{arch}"


def cache_root() -> Path:
    if os.environ.get(CACHE_ENV):
        return Path(os.environ[CACHE_ENV])
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Caches" / "cua" / "cli"
    if os.name == "nt":
        return Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local") / "cua" / "cli"
    return Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "cua" / "cli"


def _cached(manifest: dict, key: str) -> Path:
    return cache_root() / manifest["version"] / key


def _download(url: str, dest: Path, size: int, sha256: str) -> None:
    h = hashlib.sha256()
    got = 0
    try:
        with urllib.request.urlopen(url, timeout=60) as r, dest.open("wb") as out:
            while chunk := r.read(1 << 20):
                got += len(chunk)
                if got > size:
                    raise FetchError(f"{url} is larger than the {size} bytes the wheel expects")
                h.update(chunk)
                out.write(chunk)
    except OSError as e:
        raise FetchError(f"cannot download {url}: {e}") from e
    if got != size or h.hexdigest() != sha256:
        raise FetchError(
            f"{url} does not match this cua-cli release (sha256 {h.hexdigest()}, expected {sha256})"
        )


def _unpack(archive: Path, dest: Path) -> None:
    """Unpack the flat release archive: regular files only, no paths."""
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as z:
            for info in z.infolist():
                if info.is_dir():
                    continue
                if Path(info.filename).name != info.filename or info.filename.startswith("."):
                    raise FetchError(f"unexpected entry {info.filename!r} in {archive.name}")
                with z.open(info) as src, (dest / info.filename).open("wb") as out:
                    shutil.copyfileobj(src, out)
    else:
        with tarfile.open(archive, "r:gz") as t:
            for m in t.getmembers():
                name = m.name[2:] if m.name.startswith("./") else m.name
                if m.isdir() and name in ("", "."):
                    continue
                if not m.isfile() or Path(name).name != name or name.startswith("."):
                    raise FetchError(f"unexpected entry {m.name!r} in {archive.name}")
                src = t.extractfile(m)
                with (dest / name).open("wb") as out:
                    shutil.copyfileobj(src, out)
                (dest / name).chmod(0o755)


def fetch(manifest: dict | None = None, *, quiet: bool = False) -> Path:
    """Download, verify and cache this platform's CLI; return the binary."""
    manifest = manifest or release()
    if manifest is None:
        raise FetchError("this cua-cli has no release manifest to download from")
    key = platform_key()
    asset = manifest["assets"].get(key)
    if asset is None:
        raise FetchError(f"cua-cli {manifest['version']} has no build for {key}")
    final = _cached(manifest, key)
    exe = final / EXE
    if exe.is_file() and (final / VERIFIED).is_file() and (final / VERIFIED).read_text().strip() == asset["sha256"]:
        return exe

    base = os.environ.get(BASE_ENV) or manifest["base_url"]
    url = base.rstrip("/") + "/" + asset["name"]
    final.parent.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix=f".{key}-", dir=final.parent))
    try:
        if not quiet:
            mb = asset["size"] / 1e6
            sys.stderr.write(f"cua: downloading the cua CLI {manifest['version']} for {key} ({mb:.0f} MB, once)...\n")
        archive = work / asset["name"]
        _download(url, archive, asset["size"], asset["sha256"])
        unpacked = work / "out"
        unpacked.mkdir()
        _unpack(archive, unpacked)
        if not (unpacked / EXE).is_file():
            raise FetchError(f"{asset['name']} has no {EXE}")
        (unpacked / VERIFIED).write_text(asset["sha256"] + "\n")
        if final.exists():
            shutil.rmtree(final, ignore_errors=True)
        try:
            os.replace(unpacked, final)
        except OSError:
            # Another process finished first; use its copy if it is complete.
            if not ((final / EXE).is_file() and (final / VERIFIED).is_file()):
                raise
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return exe


def binary() -> Path:
    """Path of the `cua` executable: $CUA_CLI_BINARY, the bundled binary, or
    the verified download of this release (fetched on first use)."""
    override = os.environ.get(BINARY_ENV)
    if override:
        return Path(override)
    if bundled().is_file():
        return bundled()
    if release() is not None:
        return fetch()
    return bundled()
