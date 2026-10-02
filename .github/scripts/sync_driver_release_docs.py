#!/usr/bin/env python3
"""Synchronize generated Cua Driver reference-doc versions for a release branch."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
from typing import Sequence

from release_reference_docs import reference_paths


# The CLI-shape oracle the generator writes next to the pages; it records
# the documented version too.
CLI_SPEC = "scripts/docs-generators/cli-specs/cua-driver.json"


def driver_reference_paths(root: Path) -> tuple[str, ...]:
    return reference_paths(root, "cua-driver")


def replace_once(content: str, pattern: str, replacement: str, path: Path) -> str:
    updated, count = re.subn(pattern, replacement, content, flags=re.MULTILINE)
    if count != 1:
        raise RuntimeError(f"expected one release-version marker in {path}; found {count}")
    return updated


def sync_driver_release_docs(root: Path) -> None:
    version = (root / "libs/cua-driver/rust/VERSION").read_text().strip()
    for relative in driver_reference_paths(root):
        path = root / relative
        content = path.read_text()
        content = replace_once(content, r"^  Version: \S+$", f"  Version: {version}", path)
        path.write_text(content)
    spec = root / CLI_SPEC
    if spec.exists():
        content = replace_once(spec.read_text(), r'^ "version": "[^"]*"$', f' "version": "{version}"', spec)
        spec.write_text(content)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)
    try:
        sync_driver_release_docs(args.repo_root.resolve())
    except (OSError, RuntimeError) as error:
        print(f"Cua Driver release docs error: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
