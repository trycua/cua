"""Locate, download and verify the `cua-driver` executable for this machine.

The PyPI wheel is thin: it carries the SDK library (what `import cua_driver`
loads) but not the executable. `_release.json` names the signed release
archive of this exact Cua Driver version for this platform, with its size and
SHA-256 (taken from the release's checksums when the wheel was built). The
first `cua-driver` run downloads that archive, checks the hash and installs
it; later runs start the installed copy directly.

macOS: the archive holds the Developer ID signed, notarized `CuaDriver.app`.
It is installed at `/Applications/CuaDriver.app`, the same path, bundle id
(`com.trycua.driver`) and signature as the install script, so Accessibility and
Screen Recording grants are shared with a normal install and survive updates.
An existing `/Applications/CuaDriver.app` is used as it is and never
overwritten. A downloaded app is installed only after its code signature
verifies against Cua's team requirement.

Linux and Windows: the `-binary` archive is unpacked into a per-version cache.

Overrides (offline, air-gapped and pinned installs):
  CUA_DRIVER_BINARY         run this executable; nothing is downloaded
  CUA_DRIVER_DOWNLOAD_BASE  fetch archives from this URL (a mirror, or
                            file:///dir/ holding them); the hashes still apply
  CUA_DRIVER_CACHE_DIR      where the Linux and Windows cache lives

`python -m cua_driver.fetch fetch` installs ahead of time (Docker images, CI);
`python -m cua_driver.fetch path` prints the executable `cua-driver` would run.

A wheel built with the executable inside (`cua_driver/bin/`, local builds)
uses it and never downloads.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXE = "cua-driver.exe" if os.name == "nt" else "cua-driver"
BINARY_ENV = "CUA_DRIVER_BINARY"
BASE_ENV = "CUA_DRIVER_DOWNLOAD_BASE"
CACHE_ENV = "CUA_DRIVER_CACHE_DIR"
VERIFIED = ".sha256"

APP_PATH = Path("/Applications/CuaDriver.app")
APP_EXE = APP_PATH / "Contents" / "MacOS" / "cua-driver"
APP_BUNDLE_ID = "com.trycua.driver"
# Cua AI, Inc. Developer ID team. The code requirement every downloaded app
# must satisfy before it is moved into /Applications.
TEAM_ID = "YCK386LBJ7"
APP_REQUIREMENT = (
    f'=anchor apple generic and identifier "{APP_BUNDLE_ID}" '
    f'and certificate leaf[subject.OU] = "{TEAM_ID}"'
)
# Files of a `-binary` archive that the cache does not need: the SDK library
# and header already ship in the wheel, and the Node runtime is for npm.
_SKIP_SUFFIXES = (".dylib", ".so", ".dll", ".node", ".h")


class FetchError(RuntimeError):
    """The executable could not be downloaded, verified or installed."""


def bundled() -> Path:
    """Where a wheel built with the executable inside installs it."""
    return HERE / "bin" / EXE


def release() -> dict | None:
    """The `_release.json` manifest of a thin wheel, if any."""
    path = HERE / "_release.json"
    return json.loads(path.read_text()) if path.is_file() else None


def platform_key() -> str:
    """This machine's key in the manifest, e.g. `darwin-universal`, `linux-x86_64`."""
    machine = platform.machine().lower()
    arch = {"x86_64": "x86_64", "amd64": "x86_64", "arm64": "arm64", "aarch64": "arm64"}.get(machine)
    if arch is None:
        raise FetchError(f"no Cua Driver build for {sys.platform}/{machine}")
    if sys.platform == "darwin":
        return "darwin-universal"
    if sys.platform.startswith("linux"):
        if platform.libc_ver()[0] not in ("glibc", ""):
            raise FetchError("Cua Driver needs glibc 2.31 or newer (musl is not supported)")
        return f"linux-{arch}"
    if sys.platform == "win32":
        return f"windows-{arch}"
    raise FetchError(f"no Cua Driver build for {sys.platform}/{machine}")


def cache_root() -> Path:
    if os.environ.get(CACHE_ENV):
        return Path(os.environ[CACHE_ENV])
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Caches" / "cua-driver" / "python"
    if os.name == "nt":
        return Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local") / "cua-driver" / "python"
    return Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "cua-driver" / "python"


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
            f"{url} does not match this cua-driver release (sha256 {h.hexdigest()}, expected {sha256})"
        )


def _safe_relative(name: str, archive: str) -> Path:
    path = Path(name)
    if path.is_absolute() or ".." in path.parts or "\\" in name or not path.parts:
        raise FetchError(f"unexpected entry {name!r} in {archive}")
    return path


def _unpack_binary(archive: Path, dest: Path) -> None:
    """Unpack a `-binary` archive: regular files only, relative paths only."""
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as z:
            for info in z.infolist():
                if info.is_dir():
                    continue
                rel = _safe_relative(info.filename, archive.name)
                if rel.suffix.lower() in _SKIP_SUFFIXES:
                    continue
                target = dest / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                with z.open(info) as src, target.open("wb") as out:
                    shutil.copyfileobj(src, out)
        return
    with tarfile.open(archive, "r:gz") as t:
        for m in t.getmembers():
            name = m.name[2:] if m.name.startswith("./") else m.name
            if m.isdir():
                continue
            rel = _safe_relative(name, archive.name)
            if not m.isfile():
                raise FetchError(f"unexpected entry {m.name!r} in {archive.name}")
            if rel.suffix in _SKIP_SUFFIXES:
                continue
            target = dest / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            src = t.extractfile(m)
            with target.open("wb") as out:
                shutil.copyfileobj(src, out)
            target.chmod(0o755 if m.mode & 0o111 else 0o644)


def _unpack_app(archive: Path, dest: Path) -> Path:
    """Unpack only `CuaDriver.app` of the macOS archive into `dest`; return it."""
    with tarfile.open(archive, "r:gz") as t:
        for m in t.getmembers():
            rel = _safe_relative(m.name[2:] if m.name.startswith("./") else m.name, archive.name)
            parts = rel.parts
            if "CuaDriver.app" not in parts:
                continue
            inner = Path(*parts[parts.index("CuaDriver.app"):])
            target = dest / inner
            if m.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif m.issym():
                link = Path(m.linkname)
                if link.is_absolute() or ".." in link.parts:
                    raise FetchError(f"unexpected symlink {m.name!r} in {archive.name}")
                target.parent.mkdir(parents=True, exist_ok=True)
                os.symlink(m.linkname, target)
            elif m.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                with target.open("wb") as out:
                    shutil.copyfileobj(t.extractfile(m), out)
                target.chmod(m.mode & 0o777)
            else:
                raise FetchError(f"unexpected entry {m.name!r} in {archive.name}")
    app = dest / "CuaDriver.app"
    if not (app / "Contents" / "MacOS" / "cua-driver").is_file():
        raise FetchError(f"{archive.name} has no CuaDriver.app")
    return app


def _verify_app_signature(app: Path) -> None:
    """The app must be intact and signed by Cua's Developer ID team."""
    try:
        result = subprocess.run(
            ["/usr/bin/codesign", "--verify", "--deep", "--strict", "-R", APP_REQUIREMENT, str(app)],
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError) as e:
        raise FetchError(f"cannot check the code signature of the downloaded app: {e}") from e
    if result.returncode != 0:
        raise FetchError(
            "the downloaded CuaDriver.app is not signed by Cua AI, Inc.; refusing to install it "
            f"({result.stderr.strip() or 'codesign failed'})"
        )


def installed_app_version() -> str | None:
    """Version of the app at /Applications/CuaDriver.app, if it is installed."""
    try:
        with (APP_PATH / "Contents" / "Info.plist").open("rb") as f:
            return plistlib.load(f).get("CFBundleShortVersionString")
    except (OSError, plistlib.InvalidFileException, ValueError):
        return None


def _version_tuple(version: str | None) -> tuple[int, ...] | None:
    try:
        return tuple(int(p) for p in str(version).split("-")[0].split("+")[0].split("."))
    except ValueError:
        return None


def _asset(manifest: dict) -> tuple[str, dict]:
    key = platform_key()
    asset = manifest["assets"].get(key)
    if asset is None:
        raise FetchError(f"cua-driver {manifest['version']} has no build for {key}")
    return key, asset


def _fetch_archive(manifest: dict, key: str, asset: dict, work: Path, quiet: bool) -> Path:
    base = os.environ.get(BASE_ENV) or manifest["base_url"]
    url = base.rstrip("/") + "/" + asset["name"]
    if not quiet:
        mb = asset["size"] / 1e6
        sys.stderr.write(f"cua-driver: downloading Cua Driver {manifest['version']} for {key} ({mb:.0f} MB, once)...\n")
    archive = work / asset["name"]
    _download(url, archive, asset["size"], asset["sha256"])
    return archive


def install_app(manifest: dict, *, quiet: bool = False) -> Path:
    """Install the signed CuaDriver.app at /Applications; return its executable.

    An existing app is left alone, whatever its version: it holds the user's
    permission grants and is updated with `cua-driver update`.
    """
    if APP_EXE.is_file():
        installed = _version_tuple(installed_app_version())
        wanted = _version_tuple(manifest["version"])
        if not quiet and installed and wanted and installed < wanted:
            sys.stderr.write(
                f"cua-driver: {APP_PATH} is {installed_app_version()}, older than this package "
                f"({manifest['version']}). It keeps working and keeps its permissions; "
                "run `cua-driver update --apply` to update it.\n"
            )
        return APP_EXE
    key, asset = _asset(manifest)
    parent = APP_PATH.parent
    if not os.access(parent, os.W_OK):
        raise FetchError(
            f"cannot write to {parent}. Install Cua Driver once with "
            "`curl -fsSL https://cua.ai/driver/install.sh | bash`, or run this as an administrator."
        )
    # Staged inside /Applications so the final rename stays on one volume.
    work = Path(tempfile.mkdtemp(prefix=".CuaDriver-install-", dir=parent))
    try:
        archive = _fetch_archive(manifest, key, asset, work, quiet)
        staged = _unpack_app(archive, work / "out")
        _verify_app_signature(staged)
        try:
            os.rename(staged, APP_PATH)
        except OSError:
            # Another process installed it first; use that copy if it is complete.
            if not APP_EXE.is_file():
                raise
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return APP_EXE


def fetch(manifest: dict | None = None, *, quiet: bool = False) -> Path:
    """Install (macOS) or cache (Linux, Windows) the executable; return its path."""
    manifest = manifest or release()
    if manifest is None:
        raise FetchError("this cua-driver has no release manifest to download from")
    if sys.platform == "darwin":
        return install_app(manifest, quiet=quiet)
    key, asset = _asset(manifest)
    final = cache_root() / manifest["version"] / key
    exe = final / EXE
    marker = final / VERIFIED
    if exe.is_file() and marker.is_file() and marker.read_text().strip() == asset["sha256"]:
        return exe
    final.parent.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix=f".{key}-", dir=final.parent))
    try:
        archive = _fetch_archive(manifest, key, asset, work, quiet)
        unpacked = work / "out"
        unpacked.mkdir()
        _unpack_binary(archive, unpacked)
        if not (unpacked / EXE).is_file():
            raise FetchError(f"{asset['name']} has no {EXE}")
        (unpacked / VERIFIED).write_text(asset["sha256"] + "\n")
        if final.exists():
            shutil.rmtree(final, ignore_errors=True)
        try:
            os.replace(unpacked, final)
        except OSError:
            # Another process finished first; use its copy if it is complete.
            if not (exe.is_file() and marker.is_file()):
                raise
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return exe


def binary() -> Path:
    """Path of the `cua-driver` executable: $CUA_DRIVER_BINARY, the bundled
    executable, or the verified download of this release (fetched on first use)."""
    override = os.environ.get(BINARY_ENV)
    if override:
        return Path(override)
    if bundled().is_file():
        return bundled()
    if release() is not None:
        return fetch()
    return bundled()


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    cmd = args[0] if args else "path"
    try:
        if cmd == "fetch":
            exe = fetch() if release() is not None else binary()
        elif cmd == "path":
            exe = binary()
        else:
            sys.stderr.write("usage: python -m cua_driver.fetch [fetch|path]\n")
            return 2
    except FetchError as e:
        sys.stderr.write(f"cua-driver: {e}\n")
        return 1
    print(exe)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
