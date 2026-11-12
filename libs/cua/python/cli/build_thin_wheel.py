#!/usr/bin/env python3
"""Build the thin `cua-cli` wheel that PyPI gets (py3-none-any, a few KB).

It carries no binary. `cua_cli/_release.json` names the standalone CLI
archive of each platform on the GitHub release (`cua-cli-<version>-<platform>
.tar.gz|.zip`, the same files install.sh uses) with its size and SHA-256; the
first `cua` run downloads the one for its machine and checks it
(src/cua_cli/__init__.py). One wheel instead of six 71-82 MB platform
wheels: about 459 MB less per release on PyPI.

Run by cd-cua-sdk.yml after it packs the archives:

    python build_thin_wheel.py --version 0.26.0 --archives ../../../../release \\
        --base-url https://github.com/trycua/cua/releases/download/cua-sdk-v0.26.0/ \\
        --out-dir ../../../../release
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKAGE = HERE / "src" / "cua_cli"
# Manifest key -> archive suffix. Keys match cua_cli.platform_key().
PLATFORMS = {
    "darwin-arm64": "tar.gz",
    "darwin-x64": "tar.gz",
    "linux-x64": "tar.gz",
    "linux-arm64": "tar.gz",
    "windows-x64": "zip",
    "windows-arm64": "zip",
}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def manifest(version: str, archives: Path, base_url: str) -> dict:
    assets = {}
    for key, ext in PLATFORMS.items():
        name = f"cua-cli-{version}-{key}.{ext}"
        path = archives / name
        if not path.is_file():
            raise SystemExit(f"missing {path}: every platform's CLI archive is required")
        assets[key] = {"name": name, "size": path.stat().st_size, "sha256": sha256(path)}
    return {"version": version, "base_url": base_url.rstrip("/") + "/", "assets": assets}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--version", required=True)
    ap.add_argument("--archives", type=Path, required=True)
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--out-dir", type=Path, required=True)
    args = ap.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?", args.version):
        raise SystemExit(f"invalid version {args.version}")

    if (PACKAGE / "bin").exists():
        raise SystemExit(f"{PACKAGE / 'bin'} exists: a thin wheel must not carry the binary")
    data = manifest(args.version, args.archives, args.base_url)
    pyproject = HERE / "pyproject.toml"
    original = pyproject.read_text()
    release_json = PACKAGE / "_release.json"
    try:
        pyproject.write_text(re.sub(r'(?m)^version = ".*"$', f'version = "{args.version}"', original, count=1))
        release_json.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
        args.out_dir.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, CUA_CLI_THIN="1")
        env.pop("CUA_SDK_WHEEL_TAG", None)
        subprocess.run(
            [sys.executable, "-m", "build", "--wheel", "--outdir", str(args.out_dir.resolve())],
            cwd=HERE, env=env, check=True,
        )
    finally:
        pyproject.write_text(original)
        release_json.unlink(missing_ok=True)
        shutil.rmtree(HERE / "build", ignore_errors=True)
    # The filename carries the normalized version (0.26.0-rc.1 -> 0.26.0rc1).
    wheels = sorted(args.out_dir.glob("cua_cli-*-py3-none-any.whl"), key=lambda p: p.stat().st_mtime)
    if not wheels:
        raise SystemExit(f"no py3-none-any cua-cli wheel in {args.out_dir}")
    wheel = wheels[-1]
    print(f"built {wheel} ({wheel.stat().st_size} bytes) for {len(data['assets'])} platforms")


if __name__ == "__main__":
    main()
