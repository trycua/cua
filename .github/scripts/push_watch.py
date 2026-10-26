#!/usr/bin/env python3
"""Flag suspicious pushes to this repository from its Events feed.

Why a scheduled job: a push workflow runs the workflow files of the pushed
commit. A branch with someone else's history brings its own workflows and never
runs ours, so the check has to run from the default branch on a schedule.

What it flags (non-default branches only):
  * foreign history: the pushed head has no common ancestor with main. The
    13 Oct 2026 push of trycua/cloud history to this repo looked like this;
  * a very large push: more than --max-commits commits in one push, unless the
    pusher is a bot (release automation).

A finding fails the run (annotations + job summary) so the failure reaches the
usual notifications. It prints refs, SHAs and pushers, never commit content.

Usage:
  push_watch.py --repo trycua/cua --since-last-run push-watch.yml
  push_watch.py --repo trycua/cua --minutes 180        # local run, needs gh auth
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys

ZERO = "0" * 40
DEFAULT_MINUTES = 30
OVERLAP_MINUTES = 3
MAX_LOOKBACK_MINUTES = 6 * 60


def gh_api(path: str) -> tuple[int, object]:
    """GET through the gh CLI. Returns (http status, parsed body or None)."""
    proc = subprocess.run(["gh", "api", path], capture_output=True, text=True)
    if proc.returncode == 0:
        return 200, json.loads(proc.stdout or "null")
    match = re.search(r"HTTP (\d{3})", proc.stderr)
    return (int(match.group(1)) if match else 0), None


def is_bot(login: str) -> bool:
    return login.endswith("[bot]")


def classify(event: dict, repo: str, max_commits: int, compare, orphans: frozenset = frozenset()) -> list[str]:
    """Return the reasons a PushEvent is suspicious (empty when it is fine).

    `compare(base, head)` returns (status, body) like the compare API. `orphans`
    names branches that are expected to have no common ancestor with main
    (bot-maintained state branches).
    """
    payload = event.get("payload", {})
    ref = payload.get("ref", "")
    head = payload.get("head", "")
    before = payload.get("before", ZERO)
    if not ref.startswith("refs/heads/") or ref == "refs/heads/main" or not head:
        return []

    actor = event.get("actor", {}).get("login", "unknown")
    status, vs_main = compare("main", head)
    if status != 200:
        if ref.removeprefix("refs/heads/") in orphans:
            return []
        return [
            f"{ref} (head {head[:10]}) pushed by {actor} has no common ancestor with "
            f"main, or its commits cannot be read (HTTP {status}). Treat it as foreign history."
        ]

    commits = vs_main.get("ahead_by", 0)
    if before != ZERO:
        inc_status, incremental = compare(before, head)
        if inc_status == 200:
            commits = incremental.get("total_commits", commits)

    if commits > max_commits and not is_bot(actor):
        return [
            f"{ref} (head {head[:10]}) pushed by {actor} carries {commits} commits in one "
            f"push (limit {max_commits})."
        ]
    return []


def parse_time(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def window_start(args, now: dt.datetime) -> dt.datetime:
    minutes = args.minutes
    if minutes is None and args.since_last_run:
        status, runs = gh_api(
            f"repos/{args.repo}/actions/workflows/{args.since_last_run}/runs?per_page=5"
        )
        current = os.environ.get("GITHUB_RUN_ID", "")
        if status == 200:
            for run in runs.get("workflow_runs", []):
                if str(run["id"]) != current:
                    gap = (now - parse_time(run["created_at"])).total_seconds() / 60
                    minutes = min(gap + OVERLAP_MINUTES, MAX_LOOKBACK_MINUTES)
                    break
    return now - dt.timedelta(minutes=minutes or DEFAULT_MINUTES)


def recent_push_events(repo: str, since: dt.datetime) -> list[dict]:
    events: list[dict] = []
    for page in (1, 2, 3):
        status, body = gh_api(f"repos/{repo}/events?per_page=100&page={page}")
        if status != 200 or not body:
            break
        events.extend(
            e for e in body if e.get("type") == "PushEvent" and parse_time(e["created_at"]) >= since
        )
    return events


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--max-commits", type=int, default=300)
    parser.add_argument("--minutes", type=float, help="look back this many minutes")
    parser.add_argument("--since-last-run", metavar="WORKFLOW_FILE")
    parser.add_argument(
        "--orphan-branches",
        metavar="FILE",
        help="file with one branch name per line that may have no common ancestor with main",
    )
    args = parser.parse_args()

    now = dt.datetime.now(dt.timezone.utc)
    since = window_start(args, now)

    def compare(base: str, head: str):
        return gh_api(f"repos/{args.repo}/compare/{base}...{head}")

    orphans = frozenset()
    if args.orphan_branches:
        with open(args.orphan_branches, encoding="utf-8") as fh:
            orphans = frozenset(
                line.strip() for line in fh if line.strip() and not line.startswith("#")
            )

    events = recent_push_events(args.repo, since)
    findings = [
        reason
        for event in events
        for reason in classify(event, args.repo, args.max_commits, compare, orphans)
    ]

    print(f"push-watch: {len(events)} push events since {since:%Y-%m-%d %H:%M}Z, {len(findings)} suspicious")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(f"### Push watch\n\n{len(events)} push events checked, {len(findings)} suspicious.\n\n")
            fh.writelines(f"- {reason}\n" for reason in findings)
    for reason in findings:
        print(f"::error title=Suspicious push::{reason}")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
