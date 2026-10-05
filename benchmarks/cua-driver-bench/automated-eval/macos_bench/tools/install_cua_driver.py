#!/usr/bin/env python3
"""Install the pinned Cua Driver release for arm A as a private copy.

Downloads the release assets named in pins.json from github.com/trycua/cua, checks their sha256 against pins.json
and against the release's SHA256SUMS, and unpacks them under $CDB_BENCH_WORK/cua-0.34.0/. It never touches an
installed Cua Driver (the app in /Applications or ~/.local/bin). The harness calls the private binary by absolute path.

  install_cua_driver.py [--work DIR] [--verify-only]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
PINS = json.loads((HERE.parent / "pins.json").read_text("utf-8"))
BASE = (
    "https://github.com/trycua/cua/releases/download/cua-driver-rs-v" + PINS["cua_driver_version"]
)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(name: str, dest: Path) -> None:
    if dest.exists():
        return
    print(f"downloading {name}", file=sys.stderr)
    with urllib.request.urlopen(f"{BASE}/{name}", timeout=120) as r, dest.open("wb") as f:
        shutil.copyfileobj(r, f)


def safe_extract(tar_path: Path, dest: Path) -> None:
    with tarfile.open(tar_path) as tar:
        for member in tar.getmembers():
            target = (dest / member.name).resolve()
            if not str(target).startswith(str(dest.resolve())):
                raise SystemExit(f"unsafe path in archive: {member.name}")
        tar.extractall(dest)


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "--work",
        type=Path,
        default=Path(os.environ.get("CDB_BENCH_WORK") or Path.home() / ".cache" / "cua-bench-h2h"),
    )
    ap.add_argument(
        "--verify-only",
        action="store_true",
        help="check files that are already there, download nothing",
    )
    args = ap.parse_args()
    root = args.work.expanduser() / f"cua-{PINS['cua_driver_version']}"
    dl = root / "dl"
    dl.mkdir(parents=True, exist_ok=True)
    tar_name, skills_name = PINS["cua_driver_tarball"], PINS["cua_skills_tarball"]
    if not args.verify_only:
        for name in (tar_name, skills_name, "SHA256SUMS"):
            fetch(name, dl / name)
    sums = {}
    for line in (dl / "SHA256SUMS").read_text("utf-8").splitlines():
        parts = line.split()
        if len(parts) == 2:
            sums[parts[1].lstrip("*")] = parts[0]
    ok = True
    for name, pin in (
        (tar_name, PINS["cua_driver_tarball_sha256"]),
        (skills_name, PINS["cua_skills_tarball_sha256"]),
    ):
        got = sha256(dl / name)
        release = sums.get(name)
        good = got == pin and (release is None or got == release)
        ok &= good
        print(
            f"{'ok  ' if good else 'FAIL'} {name} sha256 {got} (pin {pin[:12]}..., release SHA256SUMS {'matches' if release == got else release})"
        )
    if not ok:
        return 1
    if args.verify_only:
        return 0
    ex, skills_ex = root / "ex", root / "skills-ex"
    for d in (ex, skills_ex):
        shutil.rmtree(d, ignore_errors=True)
        d.mkdir(parents=True)
    safe_extract(dl / tar_name, ex)
    safe_extract(dl / skills_name, skills_ex)
    app = root / f"CuaDriver-{PINS['cua_driver_version']}.app"
    shutil.rmtree(app, ignore_errors=True)
    shutil.copytree(next(ex.glob("*/CuaDriver.app")), app, symlinks=True)
    binary = app / "Contents/MacOS/cua-driver"
    version = subprocess.run(
        [str(binary), "--version"], capture_output=True, text=True, timeout=30
    ).stdout.strip()
    got = sha256(binary)
    print(f"{version}; binary sha256 {got}")
    if version != PINS["cua_driver_version_string"] or got != PINS["cua_driver_binary_sha256"]:
        print("FAIL: version or binary hash differs from pins.json", file=sys.stderr)
        return 1
    print(
        f"installed at {app}. The new app may need Accessibility and Screen Recording grants (System Settings)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
