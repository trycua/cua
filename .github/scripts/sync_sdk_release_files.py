#!/usr/bin/env python3
"""Regenerate every file the cua SDK version is stamped into, on a cua-sdk
release branch (run by .github/workflows/release-sync-generated-files.yml).

Release Please bumps libs/cua/VERSION and the SDK manifests. Anything that
copies that version is left behind, and main drifts once the release PR
merges (cua 0.3.0 and 0.3.1 both did). This rewrites exactly that set, with
the commands the drift checks print:

1. the generated references that record the SDK version
   (``runner.ts --library <generator>`` for each of GENERATORS);
2. the hand-maintained version facts that
   docs/scripts/tests/test_fleet_docs_version_facts.py checks (HAND_FACTS);
3. the lockfiles that lock the local ``cua`` packages: every uv.lock whose
   ``cua`` comes from the checkout (``uv lock --upgrade-package cua``) and
   every package-lock.json that links libs/cua/typescript
   (``npm install --package-lock-only --ignore-scripts``).

Nothing else is upgraded. Prints the changed paths; staging and committing
are the workflow's.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path
from typing import Sequence

# The docs generators whose pages carry the cua SDK version (from its VERSION,
# pyproject, __init__ or package.json). Each is a Check Docs row.
GENERATORS = (
    "cua-sdk",
    "cua-sdk-python",
    "cua-sdk-ts",
    "cua-rust",
    "cua-cli",
    "fleet",
    "sandbox",
)

# Hand-written pages (no generator) that state the SDK version: (page under
# docs/content/docs, regex with one version group, replacement template).
VERSION = r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?"
HAND_FACTS = (
    ("fleets/quickstart.mdx", rf"@trycua/cua@{VERSION}\b", "@trycua/cua@{version}"),
    ("fleets/guides/images.mdx", rf"`cua` {VERSION} CLI", "`cua` {version} CLI"),
)

SDK_TS = "libs/cua/typescript"
TSX = "docs/node_modules/tsx/dist/cli.mjs"


def sdk_version(root: Path) -> str:
    return (root / "libs/cua/VERSION").read_text().strip()


def sync_hand_facts(root: Path, version: str) -> list[str]:
    changed = []
    for page, pattern, template in HAND_FACTS:
        path = root / "docs/content/docs" / page
        text = path.read_text()
        updated, count = re.subn(pattern, template.format(version=version), text)
        if count == 0:
            raise RuntimeError(f"no cua version fact matching {pattern!r} in {path}")
        if updated != text:
            path.write_text(updated)
            changed.append(path.relative_to(root).as_posix())
    return changed


def tracked(root: Path, pattern: str) -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", pattern, f"**/{pattern}"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    return sorted({p for p in out.split("\0") if p})


def uv_locks_on_the_checkout(root: Path) -> list[str]:
    """uv.lock files whose `cua` package is the checkout (an editable or
    directory source), not a registry release."""
    found = []
    block = re.compile(r'^\[\[package\]\]\nname = "cua"\nversion = "[^"]*"\nsource = \{ (editable|directory) = ', re.M)
    for lock in tracked(root, "uv.lock"):
        if block.search((root / lock).read_text()):
            found.append(lock)
    return found


def npm_locks_linking_the_sdk(root: Path) -> list[str]:
    """package-lock.json files (other than the SDK's own, which Release
    Please bumps) that link libs/cua/typescript."""
    found = []
    for lock in tracked(root, "package-lock.json"):
        directory = (root / lock).parent
        if directory == root / SDK_TS:
            continue
        packages = json.loads((root / lock).read_text()).get("packages", {})
        for key in packages:
            if key and (directory / key).resolve() == (root / SDK_TS).resolve():
                found.append(lock)
                break
    return found


def run(cmd: Sequence[str], cwd: Path) -> None:
    print("+", " ".join(cmd), f"(in {cwd})", flush=True)
    subprocess.run(list(cmd), cwd=cwd, check=True)


def sync(root: Path, *, docs: bool = True, locks: bool = True) -> None:
    version = sdk_version(root)
    if docs:
        for generator in GENERATORS:
            run(["node", TSX, "scripts/docs-generators/runner.ts", "--library", generator], root)
        sync_hand_facts(root, version)
    if locks:
        for lock in uv_locks_on_the_checkout(root):
            run(["uv", "lock", "--upgrade-package", "cua"], (root / lock).parent)
        for lock in npm_locks_linking_the_sdk(root):
            run(["npm", "install", "--package-lock-only", "--ignore-scripts"], (root / lock).parent)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--list", action="store_true", help="print the generators and lockfiles, run nothing")
    args = parser.parse_args(argv)
    root = args.repo_root.resolve()
    if args.list:
        print(json.dumps({
            "version": sdk_version(root),
            "generators": list(GENERATORS),
            "hand_facts": [page for page, _, _ in HAND_FACTS],
            "uv_locks": uv_locks_on_the_checkout(root),
            "npm_locks": npm_locks_linking_the_sdk(root),
        }, indent=2))
        return 0
    try:
        sync(root)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"cua SDK release files error: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
