"""Bring uv.lock entries for in-repo packages to the version in their pyproject.

Release Please bumps a package's pyproject.toml and its own uv.lock. Every other
uv.lock that locks that package from the checkout (an `editable` or `directory`
source) still records the old version, so the release PR fails "lockfiles match
manifests" (scripts/ci/check-lockfiles.sh). That covers cua-sandbox, cua-bench
and any later package, with no per-package list.

Only the recorded version changes: rewriting the field instead of relocking
keeps the diff independent of the uv version on the runner and runs no package
tooling. Prints the changed lockfiles; staging is the caller's.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import tomllib

ENTRY = re.compile(
    r'^(\[\[package\]\]\nname = "(?P<name>[^"]+)"\nversion = ")(?P<version>[^"]*)'
    r'("\nsource = \{ (?:editable|directory) = "(?P<path>[^"]*)" \})',
    re.M,
)


def tracked_locks(root: Path) -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", "uv.lock", "**/uv.lock"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    return sorted({path for path in out.split("\0") if path})


def manifest_version(root: Path, lock: Path, name: str, path: str) -> str | None:
    pyproject = (lock.parent / path / "pyproject.toml").resolve()
    if root not in pyproject.parents or not pyproject.is_file():
        return None
    project = tomllib.loads(pyproject.read_text()).get("project", {})
    if project.get("name") != name or "version" not in project:
        return None
    return project["version"]


def sync_lock(root: Path, lock: Path) -> bool:
    text = lock.read_text()

    def replace(match: re.Match[str]) -> str:
        version = manifest_version(root, lock, match["name"], match["path"])
        if version is None or version == match["version"]:
            return match[0]
        return f'{match[1]}{version}{match[4]}'

    updated = ENTRY.sub(replace, text)
    if updated == text:
        return False
    lock.write_text(updated)
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Sync in-repo package versions in uv.lock files.")
    parser.add_argument("--repo-root", dest="root", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)
    root = args.root.resolve()
    print("\n".join(rel for rel in tracked_locks(root) if sync_lock(root, root / rel)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
