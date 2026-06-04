#!/usr/bin/env python3
"""
Add or verify SPDX + copyright headers on cua-driver source files.

Two modes:
  --apply (default): insert the header on files that lack it
  --check:           exit 1 if any tracked file is missing the header (use in CI)

Header format (matches Linux-kernel / Rust ecosystem convention):

  // SPDX-License-Identifier: MIT
  // Copyright (c) 2026 Cua AI, Inc.
  //
  // <blank line>
  // <original file content>

The script is idempotent: it skips any file that already contains an
SPDX-License-Identifier marker in its first 2 KiB.

Typical use:
  scripts/spdx-headers.py                     # apply to libs/cua-driver
  scripts/spdx-headers.py libs/cua-driver-rs  # apply to a different tree
  scripts/spdx-headers.py --check             # CI gate
  scripts/spdx-headers.py --dry-run           # preview only
"""

from __future__ import annotations

import argparse
import pathlib
import sys
from typing import Iterable

LICENSE_ID = "MIT"
COPYRIGHT_HOLDER = "Cua AI, Inc."
COPYRIGHT_YEAR = "2026"

HEADER_LINES = [
    f"// SPDX-License-Identifier: {LICENSE_ID}",
    f"// Copyright (c) {COPYRIGHT_YEAR} {COPYRIGHT_HOLDER}",
    "",
]
MARKER = "SPDX-License-Identifier"
EXTENSIONS = {".swift", ".rs"}
SKIP_DIR_NAMES = {"target", ".build", "build", "node_modules", "DerivedData", ".git"}
DEFAULT_ROOT = "libs/cua-driver"


def file_has_header(path: pathlib.Path) -> bool:
    try:
        head = path.read_bytes()[:2048].decode("utf-8", errors="replace")
    except OSError:
        return True  # don't try to rewrite unreadable files
    return MARKER in head


def insert_header(path: pathlib.Path) -> None:
    body = path.read_text(encoding="utf-8")
    path.write_text("\n".join(HEADER_LINES) + "\n" + body, encoding="utf-8")


def iter_source_files(root: pathlib.Path) -> Iterable[pathlib.Path]:
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix not in EXTENSIONS:
            continue
        if any(part in SKIP_DIR_NAMES for part in path.parts):
            continue
        yield path


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "root", nargs="?", default=DEFAULT_ROOT, help=f"directory to scan (default: {DEFAULT_ROOT})"
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--apply", action="store_true", help="insert headers on files that lack them (default)"
    )
    mode.add_argument("--check", action="store_true", help="exit 1 if any file is missing a header")
    mode.add_argument(
        "--dry-run", action="store_true", help="report what would change without writing"
    )
    args = parser.parse_args()

    root = pathlib.Path(args.root).resolve()
    if not root.exists():
        print(f"error: {root} does not exist", file=sys.stderr)
        return 2

    apply_mode = args.apply or not (args.check or args.dry_run)
    missing: list[pathlib.Path] = []
    touched = 0
    skipped = 0

    for path in iter_source_files(root):
        if file_has_header(path):
            skipped += 1
            continue
        missing.append(path)
        rel = path.relative_to(root)
        if apply_mode:
            insert_header(path)
            touched += 1
            print(f"+ {rel}")
        elif args.dry_run:
            print(f"would add header: {rel}")
        else:  # --check
            print(f"missing header: {rel}")

    print()
    if args.check:
        if missing:
            print(
                f"error: {len(missing)} file(s) missing SPDX header under {root}", file=sys.stderr
            )
            return 1
        print(f"ok: all {skipped} source files under {root} carry SPDX headers")
        return 0

    if args.dry_run:
        print(f"would add headers to {len(missing)} file(s); {skipped} already have one")
        return 0

    print(f"added headers to {touched} file(s); {skipped} already had one")
    return 0


if __name__ == "__main__":
    sys.exit(main())
