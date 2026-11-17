"""Stamp the Sandbox release version into the maintained SDK documentation.

Release Please does not touch hand-written pages, so a page that names the
previous cua-sandbox version would go stale. Install instructions carry no
version (docs/scripts/tests/test_docs_version_facts.py), because the release
PR merges before PyPI publishes the package. Only lines that name cua-sandbox
change, because the previous version number can belong to another package
(cua-agent also released 0.9.1).
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re

SDK_DOCS = "docs/content/docs/cua-sdk"
# A retrospective that lists past versions, not a maintained fact.
SKIP = {"guides/cua-cloud-ended.mdx"}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Synchronize Sandbox release documentation.")
    parser.add_argument("--repo-root", dest="root", type=Path, default=Path.cwd())
    parser.add_argument("--from", dest="previous", required=True)
    args = parser.parse_args(argv)
    version = (args.root / "libs/python/cua-sandbox/VERSION").read_text().strip()
    base = args.root / SDK_DOCS
    old = re.compile(r"(?<![\d.])" + re.escape(args.previous) + r"(?!\.?\d)")
    for path in sorted(base.rglob("*.mdx")):
        if path.relative_to(base).as_posix() in SKIP:
            continue
        lines = path.read_text().splitlines(keepends=True)
        updated = [old.sub(version, line) if "cua-sandbox" in line else line for line in lines]
        if updated != lines:
            path.write_text("".join(updated))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
