#!/usr/bin/env python3
"""Fail when a new top-level file or folder appears outside .github/root-allowlist.txt.

The repo root should show the README first, not 40 loose files. Anything that
is tracked (or untracked and not ignored) at the top level must be listed in
the allowlist; the error says where each kind of file belongs instead.

Also fails on allowlist entries that no longer exist, so the list only shrinks
when the root does.

Usage: check_root_layout.py [repo_root]
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ALLOWLIST = ".github/root-allowlist.txt"

HINTS = (
    ((".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".mp4", ".mov"), ".github/assets/ (or docs/public/ for the docs site)"),
    ((".sh", ".py", ".mjs", ".js", ".ts"), "scripts/ (or bin/ for a command people run)"),
    ((".md", ".mdx", ".txt", ".pdf"), "docs/ (blog posts in docs/blog/, contributor notes in docs/contributing/)"),
    ((".json", ".toml", ".yaml", ".yml"), "inside the component that reads it (apps/<app>/ or libs/<lib>/), or .github/ for CI"),
)


def where(name: str) -> str:
    lower = name.lower()
    for suffixes, dest in HINTS:
        if lower.endswith(suffixes):
            return dest
    if name.startswith("."):
        return ".github/ for repo and CI config, .agents/ for agent skills and rules"
    return "apps/, libs/, scripts/ or docs/, whichever owns it"


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parents[2]).resolve()
    allow = set()
    for line in (root / ALLOWLIST).read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            allow.add(line)

    listed = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        check=True, capture_output=True, text=True,
    ).stdout
    present = {p.split("/", 1)[0] for p in listed.split("\0") if p}
    # A file deleted in the working tree but not yet staged still shows as cached.
    present = {n for n in present if (root / n).exists() or (root / n).is_symlink()}

    errors = []
    for name in sorted(present - allow):
        errors.append(
            f"  {name}: not allowed at the repo root. Put it in {where(name)}.\n"
            f"    If it truly must live at the root, add it to {ALLOWLIST} with the reason in the PR."
        )
    for name in sorted(allow - present):
        errors.append(f"  {name}: listed in {ALLOWLIST} but not in the repo. Remove the line.")
    if errors:
        print("Repo root layout check failed (keep the root to the README and standard files):")
        print("\n".join(errors))
        return 1
    print(f"root layout ok: {len(present)} top-level entries")
    return 0


if __name__ == "__main__":
    sys.exit(main())
