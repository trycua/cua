"""Bring the lockfiles that record the cua SDK version to the release version.

Release Please bumps the SDK's VERSION, pyproject, __init__, package.json and
its own package-lock.json. The other lockfiles that lock the checkout's `cua`
still record the old version, so the release PR fails "lockfiles match
manifests" (scripts/ci/check-lockfiles.sh). Those are:

- every uv.lock whose `cua` package is the checkout (editable or directory);
- every package-lock.json, other than the SDK's own, that links
  libs/cua/typescript;
- every Cargo.lock that locks crates of the libs/cua workspace by path
  (cua-proto, cua-spacesd-client, cua-hotspot, ...): the workspace releases
  at the SDK version, so those entries follow it. Release Please bumps
  libs/cua/Cargo.lock's own entries through the workspace version.

Only that one recorded version changes. Rewriting the field instead of
relocking keeps the diff independent of the uv and npm versions on the runner
(a newer uv also rewrites the lock `revision`) and runs no package tooling.
Prints the changed lockfiles; staging is the caller's.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess

SDK_TS = "libs/cua/typescript"
UV_CUA_FROM_CHECKOUT = re.compile(
    r'^(\[\[package\]\]\nname = "cua"\nversion = ")[^"]*("\nsource = \{ (?:editable|directory) = )',
    re.M,
)

# A path dependency's lock entry has no `source` line after its version.
def cargo_path_entries(names: list[str]) -> re.Pattern[str]:
    return re.compile(
        r'^(\[\[package\]\]\nname = "(?:' + "|".join(map(re.escape, names)) + r')"\nversion = ")[^"]*("\n(?!source = ))',
        re.M,
    )


def workspace_crates(root: Path) -> list[str]:
    """Package names of the libs/cua Cargo workspace."""
    names = []
    for manifest in sorted((root / "libs/cua/crates").glob("*/Cargo.toml")):
        found = re.search(r'^\[package\]\nname = "([^"]+)"', manifest.read_text(), re.M)
        if found:
            names.append(found.group(1))
    return names


def tracked(root: Path, name: str) -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", name, f"**/{name}"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    return sorted({path for path in out.split("\0") if path})


def sync_uv_lock(path: Path, version: str) -> bool:
    text = path.read_text()
    updated = UV_CUA_FROM_CHECKOUT.sub(rf"\g<1>{version}\g<2>", text)
    if updated == text:
        return False
    path.write_text(updated)
    return True


def sync_cargo_lock(path: Path, entries: re.Pattern[str], version: str) -> bool:
    text = path.read_text()
    updated = entries.sub(rf"\g<1>{version}\g<2>", text)
    if updated == text:
        return False
    path.write_text(updated)
    return True


def sdk_keys(lock: Path, sdk: Path) -> list[str]:
    packages = json.loads(lock.read_text()).get("packages", {})
    return [key for key in packages if key and (lock.parent / key).resolve() == sdk]


def sync_npm_lock(path: Path, sdk: Path, version: str) -> bool:
    text = updated = path.read_text()
    for key in sdk_keys(path, sdk):
        # The entry's own fields sit at six spaces; "version" is one of them.
        entry = re.compile(
            r'(\n    "' + re.escape(key) + r'": \{\n(?:      "(?!version")[^\n]*\n)*      "version": ")[^"]*(")'
        )
        updated, count = entry.subn(rf"\g<1>{version}\g<2>", updated)
        if count != 1:
            raise RuntimeError(f"{path}: expected one version for {key}, found {count}")
    if updated == text:
        return False
    path.write_text(updated)
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Sync the cua SDK version recorded in lockfiles.")
    parser.add_argument("--repo-root", dest="root", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)
    root = args.root.resolve()
    version = (root / "libs/cua/VERSION").read_text().strip()
    sdk = (root / SDK_TS).resolve()
    changed = [lock for lock in tracked(root, "uv.lock") if sync_uv_lock(root / lock, version)]
    for lock in tracked(root, "package-lock.json"):
        if (root / lock).parent.resolve() != sdk and sync_npm_lock(root / lock, sdk, version):
            changed.append(lock)
    names = workspace_crates(root)
    if names:
        entries = cargo_path_entries(names)
        changed += [
            lock
            for lock in tracked(root, "Cargo.lock")
            if lock != "libs/cua/Cargo.lock" and sync_cargo_lock(root / lock, entries, version)
        ]
    print("\n".join(changed))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
