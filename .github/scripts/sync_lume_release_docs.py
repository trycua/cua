#!/usr/bin/env python3
"""Synchronize generated Lume reference-doc versions for a release branch."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
from typing import Sequence


from release_reference_docs import reference_paths

BODY_MARKER = r"Documented against Lume \*\*\S+\*\*\."


def lume_reference_paths(root: Path) -> tuple[str, ...]:
    return reference_paths(root, "lume")


def replace_once(content: str, pattern: str, replacement: str, path: Path) -> str:
    updated, count = re.subn(pattern, replacement, content, flags=re.MULTILINE)
    if count != 1:
        raise RuntimeError(f"expected one release-version marker in {path}; found {count}")
    return updated


def sync_lume_release_docs(root: Path) -> None:
    version = (root / "libs/lume/VERSION").read_text().strip()
    for relative in lume_reference_paths(root):
        path = root / relative
        content = path.read_text()
        content = replace_once(content, r"^  Version: \S+$", f"  Version: {version}", path)
        if re.search(BODY_MARKER, content):
            content = replace_once(
                content, BODY_MARKER, f"Documented against Lume **{version}**.", path
            )
        path.write_text(content)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)
    try:
        sync_lume_release_docs(args.repo_root.resolve())
    except (OSError, RuntimeError) as error:
        print(f"Lume release docs error: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
