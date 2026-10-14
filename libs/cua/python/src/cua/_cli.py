"""The `cua` console command: runs the Rust CLI bundled in this wheel."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def binary() -> Path:
    """Path of the bundled `cua` executable."""
    name = "cua.exe" if os.name == "nt" else "cua"
    return Path(__file__).resolve().parent / "bin" / name


def main() -> int:
    exe = binary()
    if not exe.exists():
        sys.stderr.write(
            f"cua: the CLI binary is missing from this installation ({exe}).\n"
            "Reinstall a platform wheel of `cua`, or build it with "
            "`cargo install --locked --path libs/cua/crates/cua-cli`.\n"
        )
        return 127
    argv = [str(exe), *sys.argv[1:]]
    if os.name == "nt":
        return subprocess.call(argv)
    os.execv(argv[0], argv)
    return 0  # unreachable


if __name__ == "__main__":
    raise SystemExit(main())
