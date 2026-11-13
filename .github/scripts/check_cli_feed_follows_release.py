#!/usr/bin/env python3
"""Report when the installer feed lags the newest stable cua CLI release.

install.sh and install.ps1 read the CLI version from release-artifacts.json on
the rolling `cua-install-latest` release, which cd-cua-sdk.yml updates as its
last publish step. If that step fails after the release is published (it did
on every release 7-10 Nov 2026, #6923), installs keep choosing an old CLI while
a newer one is public. This check compares the two and prints one line per
problem; the release pipeline guard turns the output into its tracking issue.

  check_cli_feed_follows_release.py --repo trycua/cua [--grace-minutes 120]

Exit 0 when the feed is current (or a release is still inside the grace
window), 1 when it lags, 2 when the data could not be read.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone

TAG = re.compile(r"^cua-sdk-v(\d+)\.(\d+)\.(\d+)$")


def parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def newest_stable(releases: list[dict]) -> tuple[tuple[int, int, int], dict] | None:
    """The highest cua-sdk-vX.Y.Z release that is published and not a prerelease."""
    best = None
    for rel in releases:
        m = TAG.match(rel.get("tagName", ""))
        if not m or rel.get("isDraft") or rel.get("isPrerelease") or not rel.get("publishedAt"):
            continue
        key = tuple(int(p) for p in m.groups())
        if best is None or key > best[0]:
            best = (key, rel)
    return best


def feed_problems(releases: list[dict], manifest: dict, now: datetime, grace: timedelta) -> list[str]:
    """Problems between the newest stable CLI release and the rolling manifest."""
    newest = newest_stable(releases)
    if newest is None:
        return []
    key, rel = newest
    wanted = ".".join(map(str, key))
    if now - parse_time(rel["publishedAt"]) < grace:
        return []  # the feed step runs minutes after the release publishes
    problems = []
    cli = [a for a in manifest.get("artifacts", []) if a.get("component") == "cli"]
    declared = str(manifest.get("versions", {}).get("cli", ""))
    if declared != wanted:
        problems.append(
            f"cua-install-latest release-artifacts.json says CLI {declared or 'none'} but {rel['tagName']} "
            f"has been published since {rel['publishedAt']}. install.sh would still install {declared or 'no CLI'}."
        )
    stale = sorted({f"{a.get('platform')}={a.get('version')}" for a in cli if a.get("version") != wanted})
    if stale and declared == wanted:
        problems.append(f"CLI entries older than {wanted} in the rolling manifest: {', '.join(stale)}")
    return problems


def gh_json(*args: str):
    out = subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--repo", required=True)
    ap.add_argument("--grace-minutes", type=int, default=120)
    args = ap.parse_args(argv)
    try:
        releases = gh_json("release", "list", "--repo", args.repo, "--limit", "100",
                           "--json", "tagName,isDraft,isPrerelease,publishedAt")
        raw = subprocess.run(
            ["gh", "release", "download", "cua-install-latest", "--repo", args.repo,
             "--pattern", "release-artifacts.json", "--output", "-"],
            check=True, capture_output=True, text=True).stdout
        manifest = json.loads(raw)
    except (subprocess.CalledProcessError, ValueError) as exc:
        print(f"could not read the release list or the rolling manifest: {exc}")
        return 2
    problems = feed_problems(releases, manifest, datetime.now(timezone.utc),
                             timedelta(minutes=args.grace_minutes))
    for p in problems:
        print(f"- {p}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
