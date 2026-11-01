#!/usr/bin/env python3
"""Build the platform wheels of the `cua` package (around a built cua-sdk
cdylib) and of `cua-cli` (the `cua` CLI binary).

The native library comes from `--library` (a CI artifact or a local
`cargo build --release -p cua-sdk`) and the CLI from `--cli` (`cargo build
--release -p cua-cli`); by default the host builds under
libs/cua/target/release are used.

- `cua`: the library next to `_native.py`. Linux wheels also carry the
  static cua-spacesd (`--spacesd`, the `cua-spacesd-linux-<arch>` that
  .github/scripts/fetch_static_spacesd.sh fetches) in `cua/bin/`, where the
  in-process SDK finds it to inject into images that lack one.
- `cua-cli` (cli/): `cua_cli/bin/cua[.exe]`, which the `cua` wheel requires
  at its own version (added at build time; a source install has no pin) and
  whose `cua` console command runs it, pointing it at that cua-spacesd
  (`$CUA_SPACESD_STATIC`). `--no-cli` skips it.

The CLI and the library are separate wheels because each repeats most of
the Rust code and PyPI caps a file at 100 MB. Both are tagged for the
library's platform, the same way libs/cua-driver/python/build_wheel.py does
it.

Usage:
    python build_wheel.py [--library PATH] [--cli PATH|--no-cli]
                          [--spacesd PATH|--no-spacesd]
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
# The cua-cli project; its staged binary lands at cua_cli/bin/ in the wheel.
CLI_PROJECT = HERE / "cli"
CLI_BIN = CLI_PROJECT / "src" / "cua_cli" / "bin"
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
    parser.add_argument("--no-cli", action="store_true", help="skip the cua-cli wheel")
    parser.add_argument(
        "--spacesd",
        type=Path,
        help="the static cua-spacesd-linux-<arch> (required for Linux wheels)",
    )
    parser.add_argument(
        "--no-spacesd",
        action="store_true",
        help="build a Linux wheel without it (test builds: images without cua-spacesd fail)",
    )
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
    if system == "linux" and not args.no_spacesd:
        name = f"cua-spacesd-linux-{'aarch64' if arch == 'arm64' else 'x86_64'}"
        if args.spacesd is None:
            raise SystemExit(f"a Linux wheel needs --spacesd PATH (the static {name})")
        if not args.spacesd.is_file():
            raise SystemExit(f"missing static cua-spacesd {args.spacesd}")
        bin_dir.mkdir()
        shutil.copy2(args.spacesd, bin_dir / name)
        (bin_dir / name).chmod(0o755)
    elif args.spacesd is not None:
        raise SystemExit("--spacesd is for Linux wheels")

    shutil.rmtree(CLI_BIN, ignore_errors=True)
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
        CLI_BIN.mkdir(parents=True)
        shutil.copy2(cli, CLI_BIN / exe)
        (CLI_BIN / exe).chmod(0o755)

    env = dict(os.environ, CUA_SDK_WHEEL_TAG=wheel_tag(system, arch))
    # The `cua` wheel requires the `cua-cli` wheel built with it, at the same
    # version (hatch_build.py adds the pin; the source pyproject has none).
    env["CUA_SDK_WHEEL_REQUIRES_CLI"] = "0" if args.no_cli else "1"
    args.out_dir.mkdir(parents=True, exist_ok=True)
    try:
        import build  # noqa: F401

        cmd = [sys.executable, "-m", "build", "--wheel", "--outdir", str(args.out_dir.resolve())]
    except ImportError:
        uv = shutil.which("uv")
        if not uv:
            raise SystemExit("need `build` (pip install build) or `uv`")
        cmd = [uv, "build", "--wheel", "--out-dir", str(args.out_dir.resolve())]
    subprocess.run(cmd, cwd=HERE, env=env, check=True)
    print(f"built the {wheel_tag(system, arch)} cua wheel into {args.out_dir}")
    if not args.no_cli:
        subprocess.run(cmd, cwd=CLI_PROJECT, env=env, check=True)
        print(f"built the {wheel_tag(system, arch)} cua-cli wheel into {args.out_dir}")


if __name__ == "__main__":
    main()
