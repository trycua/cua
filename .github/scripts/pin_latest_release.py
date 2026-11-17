#!/usr/bin/env python3
"""Keep the repo-wide GitHub "Latest" release on the newest Cua Spaces release.

Only Cua Spaces may be Latest; every other component opts out when it
publishes. This guard runs after any release publishes (and on a schedule,
because releases created with GITHUB_TOKEN do not trigger `release` events) and
puts Latest back when another component took it. It never deletes or edits
anything else.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys

OWNER_PREFIX = "cua-spaces-v"
STABLE = re.compile(r"^cua-spaces-v(\d+)\.(\d+)\.(\d+)$")


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def newest_spaces_release(releases: list[dict]) -> dict | None:
    stable = [
        r
        for r in releases
        if not r.get("draft") and not r.get("prerelease") and STABLE.match(r.get("tag_name", ""))
    ]
    return max(stable, key=lambda r: tuple(map(int, STABLE.match(r["tag_name"]).groups())), default=None)


def main() -> int:
    repo = os.environ.get("GITHUB_REPOSITORY", "trycua/cua")
    listing = json.loads(
        gh("release", "list", "--repo", repo, "--limit", "30", "--json", "tagName,isLatest,isDraft,isPrerelease")
    )
    current = next((r["tagName"] for r in listing if r["isLatest"]), None)
    # Two Cua Spaces releases build at once and finish in any order, so an
    # older one can publish after a newer one and take Latest (0.37.0 did, 16 Nov 2026).
    # Latest on a Spaces release is only right when it is the newest published one.
    newest_recent = newest_spaces_release(
        [{"tag_name": r["tagName"], "draft": r["isDraft"], "prerelease": r["isPrerelease"]} for r in listing]
    )
    if current and current.startswith(OWNER_PREFIX) and (newest_recent is None or newest_recent["tag_name"] == current):
        print(f"Latest is {current}; nothing to do")
        return 0
    releases = json.loads(gh("api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100"))
    flat = [r for page in releases for r in page]
    target = newest_spaces_release(flat)
    if target is None:
        print("::warning::no published Cua Spaces release to pin Latest to")
        return 0
    print(f"::warning::Latest was {current}; resetting it to {target['tag_name']}")
    gh("api", "-X", "PATCH", f"repos/{repo}/releases/{target['id']}", "-f", "make_latest=true")
    return 0


if __name__ == "__main__":
    sys.exit(main())
