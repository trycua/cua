"""The `cua` console command: runs the Rust CLI that the `cua-cli` wheel (a
dependency of `cua`) installs at cua_cli/bin/cua[.exe]."""

from __future__ import annotations

import os
import subprocess
import sys
from importlib import metadata
from pathlib import Path

# The static cua-spacesd the Linux `cua` wheel ships (cua/bin/, where the
# in-process SDK finds it); the CLI lives in another package, so it is told.
SPACESD_ENV = "CUA_SPACESD_STATIC"


def binary() -> Path | None:
    """Path of the `cua` executable the `cua-cli` wheel installs, if any."""
    try:
        import cua_cli
    except ImportError:
        return None
    exe = cua_cli.binary()
    return exe if exe.is_file() else None


def spacesd() -> Path | None:
    """The static cua-spacesd in this wheel (Linux), if any."""
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
        "cua: the cua CLI is not installed (the cua-cli package, skipped by --no-deps). "
        f'Install it with `pip install "{pin}"`.\n'
    )


def main() -> int:
    exe = binary()
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
