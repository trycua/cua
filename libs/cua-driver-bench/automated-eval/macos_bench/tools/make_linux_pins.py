#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Write the Linux identity pin file the runner requires, from a successful doctor report."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import claude_arms as ca  # noqa: E402

CHECKED_IN = HERE.parent / "pins.json"


def in_checkout(path: Path) -> bool:
    parent = path.parent
    while not parent.exists():
        parent = parent.parent
    return (
        subprocess.run(
            ["git", "-C", str(parent), "rev-parse", "--show-toplevel"],
            capture_output=True,
        ).returncode
        == 0
    )


def build_pins(binary: Path, skills: Path, report: dict, version: str) -> dict:
    build = report.get("build") or {}
    digest = ca.sha256_file(binary)
    if not report.get("ok"):
        raise SystemExit("doctor report is not ok")
    if build.get("exe_sha256") != digest:
        raise SystemExit("doctor build exe_sha256 does not match --cua-bin")
    if build.get("version") != version:
        raise SystemExit("doctor build version does not match --cua-bin --version")
    if not build.get("git_sha"):
        raise SystemExit("doctor build has no git_sha")
    if not (skills / "SKILL.md").is_file():
        raise SystemExit("skills directory has no SKILL.md")
    tree = ca.sha256_tree(skills)
    return {
        "platform": "linux",
        "cua_driver_version": version,
        "cua_driver_version_string": f"cua-driver {version}",
        "cua_driver_git_sha": build["git_sha"],
        "cua_driver_binary_sha256": digest,
        "cua_skills_tree_sha256": tree,
        "cua_main": {
            "version": version,
            "version_string": f"cua-driver {version}",
            "git_sha": build["git_sha"],
            "binary_sha256": digest,
            "script_binary_sha256": digest,
            "skills_tree_sha256": tree,
            "note": "Linux: one explicit driver executable backs every Cua Driver arm.",
        },
        "cdb_pack": json.loads(CHECKED_IN.read_text("utf-8"))["cdb_pack"],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cua-bin", type=Path, required=True)
    parser.add_argument("--cua-skills", type=Path, required=True)
    parser.add_argument("--doctor-json", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    out = args.out.resolve()
    if in_checkout(out):
        raise SystemExit("refusing to write inside a git checkout")
    if out.exists() or args.out.is_symlink():
        raise SystemExit(f"refusing to overwrite {out}")
    report = json.loads(args.doctor_json.read_text("utf-8"))
    version = subprocess.run(
        [str(args.cua_bin), "--version"], capture_output=True, text=True, timeout=10, check=True
    ).stdout.strip()
    version = version.removeprefix("cua-driver").strip()
    pins = build_pins(args.cua_bin, args.cua_skills, report, version)
    out.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=out.parent, prefix=".pins-")
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(json.dumps(pins, indent=2) + "\n")
    os.replace(tmp, out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
