#!/usr/bin/env python3
"""Delete superseded draft releases that no CD run is working on.

Release Please opens a draft per version, and a CD run may add a second
(tagged) draft for the same version. A run that fails or is cancelled before
its publish step leaves both behind, and the next version makes new ones, so
drafts pile up (28 on 10 Nov 2026). This sweeper is the backstop.

A draft is deleted only when ALL hold:
  - its component and a stable X.Y.Z version can be read from its tag
    (`<component>-vX.Y.Z`) or its name (`<component>: vX.Y.Z`, `Cua Spaces vX.Y.Z`);
  - it is older than MIN_AGE_HOURS (24);
  - it is superseded: another release of the same component, published or a
    newer draft, has a version >= its own (a newer draft only counts when this
    one is not itself the newest of the component);
  - no CD run for its tag is queued or in progress, or started in the last
    ACTIVE_HOURS (3).
Drafts it cannot classify (no component, prerelease versions, hand-made
review inputs) are never touched. Tags are never deleted. DRY_RUN=1 only prints.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import re
import subprocess
import sys

MIN_AGE_HOURS = 24
ACTIVE_HOURS = 3
TAG = re.compile(r"^(?P<c>[a-z0-9][a-z0-9-]*)-v(?P<v>\d+\.\d+\.\d+)$")
NAME = re.compile(r"^(?P<c>[a-z0-9][a-z0-9-]*): v(?P<v>\d+\.\d+\.\d+)$")
SPACES_NAME = re.compile(r"^Cua Spaces v(?P<v>\d+\.\d+\.\d+)$")


def identify(rel: dict) -> tuple[str, tuple[int, ...]] | None:
    """(component, version) of a release, or None when it cannot be classified."""
    tag, name = rel.get("tag_name") or "", rel.get("name") or ""
    m = TAG.match(tag) or NAME.match(name)
    if m:
        return m["c"], tuple(map(int, m["v"].split(".")))
    m = SPACES_NAME.match(name)
    if m:
        return "cua-spaces", tuple(map(int, m["v"].split(".")))
    return None


def parse_time(s: str) -> dt.datetime:
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def active_branches(runs: list[dict], now: dt.datetime) -> set[str]:
    """Tags (run head branches) with a run in flight or started recently."""
    out = set()
    for r in runs:
        recent = (now - parse_time(r["created_at"])).total_seconds() < ACTIVE_HOURS * 3600
        if r.get("status") in ("queued", "in_progress", "waiting", "pending", "requested") or recent:
            out.add(r.get("head_branch") or "")
    return out


def pick_deletions(releases: list[dict], runs: list[dict], now: dt.datetime) -> list[tuple[dict, str]]:
    busy = active_branches(runs, now)
    known = [(r, identify(r)) for r in releases]
    known = [(r, i) for r, i in known if i]
    doomed = []
    for r, (comp, ver) in known:
        if not r.get("draft") or r.get("prerelease"):
            continue
        if (now - parse_time(r["created_at"])).total_seconds() < MIN_AGE_HOURS * 3600:
            continue
        if f"{comp}-v{'.'.join(map(str, ver))}" in busy or r.get("tag_name") in busy:
            continue
        reason = None
        for o, (oc, ov) in known:
            if o is r or oc != comp:
                continue
            if not o.get("draft") and ov >= ver:
                reason = f"published {o['tag_name']} >= {'.'.join(map(str, ver))}"
                break
            if o.get("draft") and ov > ver:
                reason = f"newer draft {o.get('tag_name')}"
        if reason:
            doomed.append((r, reason))
    return doomed


def gh_json(*args: str):
    out = subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def main() -> int:
    repo = os.environ.get("GITHUB_REPOSITORY", "trycua/cua")
    dry = os.environ.get("DRY_RUN") == "1"
    now = dt.datetime.now(dt.timezone.utc)
    flat = [r for p in gh_json("api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100") for r in p]
    runs = []
    for status in ("queued", "in_progress", "completed"):
        data = gh_json("api", f"repos/{repo}/actions/runs?per_page=100&status={status}&event=push")
        runs += data.get("workflow_runs", [])
    drafts = sum(1 for r in flat if r.get("draft"))
    doomed = pick_deletions(flat, runs, now)
    print(f"{drafts} drafts, {len(doomed)} superseded and inactive")
    for r, why in doomed:
        label = r.get("tag_name") or r["name"]
        print(f"{'would delete' if dry else 'deleting'} {label} ({r['name']}): {why}")
        if not dry:
            subprocess.run(["gh", "api", "-X", "DELETE", f"repos/{repo}/releases/{r['id']}"], check=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
