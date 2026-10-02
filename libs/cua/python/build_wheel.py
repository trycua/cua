#!/usr/bin/env python3
"""Build a platform wheel of the `cua` package around a built cua-sdk cdylib
and the `cua` CLI binary.

The native library comes from `--library` (a CI artifact or a local
`cargo build --release -p cua-sdk`) and the CLI from `--cli` (`cargo build
--release -p cua-cli`); by default the host builds under
libs/cua/target/release are used. The CLI is installed as the `cua` console
command. The wheel is tagged for the library's platform, the same way
libs/cua-driver/python/build_wheel.py does it.

Usage:
    python build_wheel.py [--library PATH] [--cli PATH|--no-cli]
                          [--platform darwin|linux|windows]
                          [--arch x86_64|arm64|universal] [--out-dir DIR]
"""

from __future__ import annotations

import argparse
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKAGE = HERE / "src" / "cua"
LIB_NAMES = {
    "darwin": "libcua_sdk.dylib",
    "linux": "libcua_sdk.so",
    "windows": "cua_sdk.dll",
}


def host_platform() -> tuple[str, str]:
    system = platform.system().lower()
    machine = platform.machine().lower()
    arch = {"amd64": "x86_64", "x86_64": "x86_64", "arm64": "arm64", "aarch64": "arm64"}.get(
        machine
    )
    if system not in LIB_NAMES or arch is None:
        raise SystemExit(f"unsupported host {system}/{machine}")
    return system, arch


def wheel_tag(system: str, arch: str) -> str:
    if system == "darwin":
        # Keep aligned with the dylib's LC_BUILD_VERSION minimum (11.0 for
        # Rust's aarch64-apple-darwin default; universal2 covers both).
        if arch == "universal":
            return "py3-none-macosx_11_0_universal2"
        return "py3-none-macosx_11_0_arm64" if arch == "arm64" else "py3-none-macosx_10_12_x86_64"
    if system == "linux":
        # Release builds run in debian:11 (glibc 2.31).
        return f"py3-none-manylinux_2_31_{'aarch64' if arch == 'arm64' else 'x86_64'}"
    if system == "windows":
        return "py3-none-win_arm64" if arch == "arm64" else "py3-none-win_amd64"
    raise SystemExit(f"unsupported wheel target {system}-{arch}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--library", type=Path)
    parser.add_argument("--cli", type=Path, help="the built `cua` CLI binary")
    parser.add_argument("--no-cli", action="store_true", help="build without the CLI binary")
    parser.add_argument("--platform", choices=sorted(LIB_NAMES))
    parser.add_argument("--arch", choices=["x86_64", "arm64", "universal"])
    parser.add_argument("--out-dir", type=Path, default=HERE / "dist")
    args = parser.parse_args()

    system, arch = host_platform()
    system = args.platform or system
    arch = args.arch or arch
    lib_name = LIB_NAMES[system]
    library = args.library or (HERE.parent / "target" / "release" / lib_name)
    if not library.exists():
        raise SystemExit(f"missing native library {library}; run cargo build --release -p cua-sdk")

    for stale in LIB_NAMES.values():
        (PACKAGE / stale).unlink(missing_ok=True)
    shutil.copy2(library, PACKAGE / lib_name)

    bin_dir = PACKAGE / "bin"
    shutil.rmtree(bin_dir, ignore_errors=True)
    if not args.no_cli:
        exe = "cua.exe" if system == "windows" else "cua"
        cli = args.cli or (HERE.parent / "target" / "release" / exe)
        # `--cli target/debug/cua` names the binary the same way on every OS;
        # on Windows the file is `cua.exe`.
        if not cli.exists() and system == "windows" and cli.suffix.lower() != ".exe":
            cli = cli.with_name(cli.name + ".exe")
        if not cli.exists():
            raise SystemExit(
                f"missing CLI binary {cli}; run cargo build --release -p cua-cli or pass --no-cli"
            )
        bin_dir.mkdir()
        shutil.copy2(cli, bin_dir / exe)
        (bin_dir / exe).chmod(0o755)

    env = dict(os.environ, CUA_SDK_WHEEL_TAG=wheel_tag(system, arch))
    args.out_dir.mkdir(parents=True, exist_ok=True)
    try:
        import build  # noqa: F401

        cmd = [sys.executable, "-m", "build", "--wheel", "--outdir", str(args.out_dir)]
    except ImportError:
        uv = shutil.which("uv")
        if not uv:
            raise SystemExit("need `build` (pip install build) or `uv`")
        cmd = [uv, "build", "--wheel", "--out-dir", str(args.out_dir)]
    subprocess.run(cmd, cwd=HERE, env=env, check=True)
    print(f"built {wheel_tag(system, arch)} wheel into {args.out_dir}")


if __name__ == "__main__":
    main()
