#!/usr/bin/env python3
"""Pick the release tags the downloads CDN backfill copies.

Explicit --tags wins. Otherwise: the three rolling tags (unless --no-rolling), the newest published
release of every product (tag prefix), and the last N stable Cua Spaces releases.
Versioned tags come first and rolling tags last, so the feeds that name them are
copied after the files they name. Prints one tag per line.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys

PRODUCTS = ["cua-sdk-v", "cua-spacesd-v", "cua-driver-rs-v"]
ROLLING = ["cua-install-latest", "cua-spaces-latest", "cua-spaces-nightly"]


def semver(tag: str, prefix: str):
    m = re.fullmatch(re.escape(prefix) + r"(\d+)\.(\d+)\.(\d+)(-.+)?", tag)
    return None if not m else (int(m[1]), int(m[2]), int(m[3]), m[4] is None)


def pick(releases: list[dict], stable_count: int, include_rolling: bool = True) -> list[str]:
    """releases: dicts with tag_name, draft. Drafts are not published and are skipped."""
    tags = [r["tag_name"] for r in releases if not r.get("draft")]
    out: list[str] = []
    spaces = sorted((t for t in tags if semver(t, "cua-spaces-v") and semver(t, "cua-spaces-v")[3]),
                    key=lambda t: semver(t, "cua-spaces-v"), reverse=True)
    out += spaces[:stable_count]
    for p in PRODUCTS:
        v = sorted((t for t in tags if semver(t, p)), key=lambda t: semver(t, p), reverse=True)
        out += v[:1]
    return list(dict.fromkeys(out + (ROLLING if include_rolling else [])))


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--tags", default="")
    ap.add_argument("--stable-count", type=int, default=1)
    ap.add_argument("--no-rolling", action="store_true", help="leave out the rolling tags")
    a = ap.parse_args(argv)
    explicit = [t for t in re.split(r"[\s,]+", a.tags) if t]
    if explicit:
        print("\n".join(explicit))
        return 0
    p = subprocess.run(["gh", "api", "--paginate", f"repos/{a.repo}/releases?per_page=100", "--jq", ".[] | {tag_name, draft}"],
                       capture_output=True, text=True)
    if p.returncode:
        print(p.stderr, file=sys.stderr)
        return 1
    rels = [json.loads(l) for l in p.stdout.splitlines() if l.strip()]
    print("\n".join(pick(rels, a.stable_count, not a.no_rolling)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
