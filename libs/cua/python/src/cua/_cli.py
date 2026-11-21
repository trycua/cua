"""The `cua` console command: runs the Rust CLI that the `cua-cli` wheel (a
dependency of `cua`) installs at cua_cli/bin/cua[.exe]."""

from __future__ import annotations

import os
import subprocess
import sys
from importlib import metadata
from pathlib import Path

# The static Linux cua-spacesd every `cua` wheel ships (cua/bin/, where the
# in-process SDK finds it); the CLI lives in another package, so it is told.
SPACESD_ENV = "CUA_SPACESD_STATIC"


def binary() -> Path | None:
    """Path of the `cua` executable the `cua-cli` wheel provides, if any.

    The PyPI `cua-cli` wheel is thin: its first use downloads the signed CLI
    of the same release and checks its SHA-256 (cua_cli.fetch), which can
    raise cua_cli.FetchError."""
    try:
        import cua_cli
    except ImportError:
        return None
    exe = cua_cli.binary()
    return exe if exe.is_file() else None


def spacesd() -> Path | None:
    """The static cua-spacesd in this wheel, if any."""
    for p in sorted((Path(__file__).resolve().parent / "bin").glob("cua-spacesd-linux-*")):
        if p.is_file():
            return p
    return None


def install_hint() -> str:
    try:
        pin = f"cua-cli=={metadata.version('cua')}"
    except metadata.PackageNotFoundError:
        pin = "cua-cli"
    return (
        "cua: the cua CLI is not installed (the cua-cli package). "
        f'Install it with `pip install "{pin}"`.\n'
    )


def fetch_failed(err: Exception) -> str:
    return (
        f"cua: {err}\n"
        "Retry when online, point CUA_CLI_DOWNLOAD_BASE at a mirror of the release "
        "archives, or set CUA_CLI_BINARY to a cua binary "
        "(https://cua.ai/install.sh installs one without Python).\n"
    )


def _fetch_error() -> type[Exception]:
    try:
        from cua_cli import FetchError
    except ImportError:  # no cua-cli, or one with the binary inside
        return RuntimeError
    return FetchError


def main() -> int:
    try:
        exe = binary()
    except _fetch_error() as err:
        sys.stderr.write(fetch_failed(err))
        return 1
    if exe is None:
        sys.stderr.write(install_hint())
        return 127
    sd = spacesd()
    if sd is not None and not os.environ.get(SPACESD_ENV):
        os.environ[SPACESD_ENV] = str(sd)
    argv = [str(exe), *sys.argv[1:]]
    if os.name == "nt":
        return subprocess.call(argv)
    os.execv(argv[0], argv)
    return 0  # unreachable


if __name__ == "__main__":
    raise SystemExit(main())
