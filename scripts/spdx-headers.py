#!/usr/bin/env python3
"""
Add or verify SPDX + copyright headers on source files.

Two trees carry headers:

  libs/cua-driver (MIT, .rs and .swift), the default root;
  every FSL-1.1-MIT package (`--fsl`): each source file whose nearest LICENSE
  file is the Functional Source License (see LICENSING.md).

Modes:
  --apply (default): insert the header on files that lack it
  --check:           exit 1 if a file is missing the header or carries the
                     wrong license id (use in CI)
  --dry-run:         report what would change without writing

Header format (Linux-kernel / Rust ecosystem convention), in the file's own
comment syntax, after any line that must stay first (a `#!` shebang, a Python
coding line, `// swift-tools-version`):

  // SPDX-License-Identifier: MIT
  // Copyright (c) 2026 Cua AI, Inc.
  //
  // <blank line>
  // <original file content>

The script is idempotent: it skips any file that already contains an
SPDX-License-Identifier marker in its first 2 KiB. Files that carry another
party's copyright notice (code adapted from elsewhere) and protobuf-generated
modules are left alone and listed.

Generators that write into an FSL package emit the header themselves
(`spdx_header()` below is the reference), so their `--check` gates hold.

Typical use:
  scripts/spdx-headers.py                     # apply to libs/cua-driver
  scripts/spdx-headers.py libs/cua-driver-rs  # apply to a different tree
  scripts/spdx-headers.py --check             # CI gate (cua-driver)
  scripts/spdx-headers.py --fsl --check       # CI gate (FSL packages)
  scripts/spdx-headers.py --dry-run           # preview only
"""

from __future__ import annotations

import argparse
import pathlib
import re
import subprocess
import sys
from typing import Iterable

COPYRIGHT_HOLDER = "Cua AI, Inc."
COPYRIGHT_YEAR = "2026"
FSL = "FSL-1.1-MIT"
MARKER = "SPDX-License-Identifier"
REPO = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_ROOT = "libs/cua-driver"
SKIP_DIR_NAMES = {"target", ".build", "build", "node_modules", "DerivedData", ".git", "dist", ".venv"}
LICENSE_NAMES = ("LICENSE", "LICENSE.md", "LICENSE.txt")

# Comment prefix per extension (a `(open, close)` pair for block-only syntaxes).
LINE = {
    ".rs": "//", ".swift": "//", ".ts": "//", ".tsx": "//", ".js": "//", ".mjs": "//",
    ".cjs": "//", ".kt": "//", ".kts": "//", ".h": "//", ".c": "//", ".m": "//",
    ".py": "#", ".sh": "#", ".ps1": "#",
}
BLOCK = {".css": ("/*", "*/")}
DRIVER_EXTENSIONS = {".swift", ".rs"}
FSL_EXTENSIONS = set(LINE) | set(BLOCK)
GENERATED_ELSEWHERE = re.compile(r"_pb2(_grpc)?\.py$|_pb\.ts$")
# Build outputs checked in beside their sources (a drift gate regenerates
# them, so a header added by hand would be overwritten): the HTML5 viewer's
# embedded bundle.
GENERATED_DIRS = ("libs/cua/crates/cua-spacesd-html5/assets/",)


def spdx_header(license_id: str, suffix: str) -> list[str]:
    """The header lines (no trailing blank line) for a file with this suffix."""
    lines = [f"SPDX-License-Identifier: {license_id}", f"Copyright (c) {COPYRIGHT_YEAR} {COPYRIGHT_HOLDER}"]
    if suffix in BLOCK:
        open_, close = BLOCK[suffix]
        return [f"{open_} {line} {close}" for line in lines]
    return [f"{LINE[suffix]} {line}" for line in lines]


def head_of(path: pathlib.Path) -> str:
    try:
        return path.read_bytes()[:2048].decode("utf-8", errors="replace")
    except OSError:
        return ""


def declared_id(head: str) -> str | None:
    match = re.search(MARKER + r":\s*([A-Za-z0-9.+-]+(?:\s+(?:AND|OR|WITH)\s+[A-Za-z0-9.+-]+)*)", head)
    return match.group(1) if match else None


def foreign_notice(head: str) -> bool:
    """Another party's copyright or license notice (adapted or vendored code)."""
    for line in head.splitlines()[:40]:
        if re.search(r"\bcopyright\b", line, re.I) and COPYRIGHT_HOLDER not in line:
            return True
        if re.search(r"licensed under", line, re.I):
            return True
    return False


def insert_header(path: pathlib.Path, license_id: str) -> None:
    body = path.read_text(encoding="utf-8")
    lines = body.split("\n")
    keep = 0
    first = lines[0] if lines else ""
    if (first.startswith("#!") and not first.startswith("#![")) or first.startswith(("// swift-tools-version", "//swift-tools-version")):
        keep = 1
    if path.suffix == ".py" and len(lines) > keep and re.match(r"#.*coding[:=]", lines[keep]):
        keep += 1
    header = spdx_header(license_id, path.suffix)
    new = lines[:keep] + header + [""] + lines[keep:]
    path.write_text("\n".join(new), encoding="utf-8")


def tracked_files(root: pathlib.Path) -> list[pathlib.Path]:
    out = subprocess.run(["git", "ls-files", "-z", "--", str(root)], cwd=REPO, capture_output=True, check=True)
    return [REPO / rel for rel in out.stdout.decode().split("\0") if rel]


def iter_source_files(root: pathlib.Path, extensions: set[str]) -> Iterable[pathlib.Path]:
    for path in tracked_files(root):
        if not path.is_file() or path.suffix not in extensions:
            continue
        if any(part in SKIP_DIR_NAMES for part in path.relative_to(REPO).parts):
            continue
        yield path


def is_fsl_license(path: pathlib.Path) -> bool:
    return "Functional Source License" in path.read_text(errors="replace")[:200]


def fsl_roots() -> list[pathlib.Path]:
    """Every directory whose LICENSE file is the FSL."""
    roots = []
    for name in LICENSE_NAMES:
        for path in tracked_files(REPO):
            if path.name == name and path.is_file() and is_fsl_license(path):
                roots.append(path.parent)
    return sorted(set(roots))


def nearest_license_is_fsl(path: pathlib.Path, cache: dict[pathlib.Path, bool]) -> bool:
    directory = path.parent
    seen = []
    while True:
        if directory in cache:
            result = cache[directory]
            break
        found = next((directory / n for n in LICENSE_NAMES if (directory / n).is_file()), None)
        if found is not None:
            result = is_fsl_license(found)
            break
        seen.append(directory)
        if directory == REPO:
            result = False
            break
        directory = directory.parent
    for d in seen:
        cache[d] = result
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("root", nargs="?", default=None, help=f"directory to scan (default: {DEFAULT_ROOT})")
    parser.add_argument("--fsl", action="store_true", help="every FSL-1.1-MIT package (by its LICENSE file)")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true", help="insert headers on files that lack them (default)")
    mode.add_argument("--check", action="store_true", help="exit 1 if any file is missing a header")
    mode.add_argument("--dry-run", action="store_true", help="report what would change without writing")
    args = parser.parse_args()

    if args.fsl:
        license_id, extensions = FSL, FSL_EXTENSIONS
        cache: dict[pathlib.Path, bool] = {}
        files = [
            path
            for root in fsl_roots()
            for path in iter_source_files(root, extensions)
            if nearest_license_is_fsl(path, cache)
        ]
        label = "FSL packages"
    else:
        root = (REPO / (args.root or DEFAULT_ROOT)).resolve()
        if not root.exists():
            print(f"error: {root} does not exist", file=sys.stderr)
            return 2
        license_id, extensions = "MIT", DRIVER_EXTENSIONS
        files = list(iter_source_files(root, extensions))
        label = str(root)

    apply_mode = args.apply or not (args.check or args.dry_run)
    missing: list[pathlib.Path] = []
    wrong: list[tuple[pathlib.Path, str]] = []
    foreign: list[pathlib.Path] = []
    touched = 0
    skipped = 0

    for path in sorted(set(files)):
        rel = path.relative_to(REPO)
        head = head_of(path)
        found = declared_id(head)
        if found is not None:
            skipped += 1
            if args.fsl and found != license_id:
                wrong.append((rel, found))
            continue
        if (
            GENERATED_ELSEWHERE.search(path.name)
            or str(rel).startswith(GENERATED_DIRS)
            or (args.fsl and foreign_notice(head))
        ):
            foreign.append(rel)
            continue
        missing.append(path)
        if apply_mode:
            insert_header(path, license_id)
            touched += 1
            print(f"+ {rel}")
        elif args.dry_run:
            print(f"would add header: {rel}")
        else:  # --check
            print(f"missing header: {rel}")

    if args.fsl:
        # The other direction: an MIT (or other non-FSL) file must never
        # carry the FSL header, or the file would read as source-available.
        covered = set(files)
        for path in tracked_files(REPO):
            if path in covered or not path.is_file() or path.suffix not in extensions:
                continue
            if any(part in SKIP_DIR_NAMES for part in path.relative_to(REPO).parts):
                continue
            if declared_id(head_of(path)) == FSL and not nearest_license_is_fsl(path, cache):
                wrong.append((path.relative_to(REPO), f"{FSL} outside an FSL package"))

    for rel, found in wrong:
        if found.endswith("outside an FSL package"):
            print(f"stray {FSL} header (the nearest LICENSE is not the FSL): {rel}")
        else:
            print(f"wrong license id ({found}, expected {license_id}): {rel}")
    if foreign:
        print(f"left alone ({len(foreign)}, another party's notice or generated elsewhere):")
        for rel in foreign:
            print(f"  {rel}")

    print()
    if args.check:
        if missing or wrong:
            print(f"error: {len(missing)} file(s) missing and {len(wrong)} with the wrong SPDX header under {label}", file=sys.stderr)
            return 1
        print(f"ok: all {skipped} source files under {label} carry SPDX headers")
        return 0

    if args.dry_run:
        print(f"would add headers to {len(missing)} file(s); {skipped} already have one")
        return 1 if wrong else 0

    print(f"added headers to {touched} file(s); {skipped} already had one")
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
