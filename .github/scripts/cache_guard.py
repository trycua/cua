#!/usr/bin/env python3
"""Fail when a main run that should have saved a build cache did not.

actions/cache saves in a post step, and a failed save is only a warning: the
job stays green, the next pull request restores nothing and builds cold (a
macOS Swift build takes 25 to 60 minutes). This runs after a main workflow run
(see .github/workflows/ci-cache-guard.yml) and checks, per expectation, that
the cache entry is there:

  --sccache JOB=PREFIX   if the job ran and succeeded, main must hold the
                         rust-cache entry PREFIX + <UTC day of the run>
                         (.github/actions/rust-cache: one key per family per day,
                         the first main run of the day saves, later runs hit it).
  --slice JOB            if the job ran and succeeded, the slice key it printed
                         in its "SwiftUI Rust slice cache hit = ... (KEY)" notice
                         (.github/actions/spaces-rust-slice) must be in main's cache.

A job that was skipped or failed is not checked (its own check already says so).
Exit 1 lists every missing entry. Needs the `gh` CLI.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys

NOTICE = re.compile(r"SwiftUI Rust slice cache hit = (?:true|false) \((swiftui-rust-[^)\s]+)\)")
MAIN = "refs/heads/main"


def gh_json(*args: str):
    out = subprocess.run(["gh", "api", *args], check=True, capture_output=True, text=True).stdout
    return json.loads(out) if out.strip() else None


def day(stamp: str) -> str:
    return dt.datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ").strftime("%Y%m%d")


def main_keys(repo: str, prefix: str) -> set[str]:
    """Keys of main's cache entries that start with prefix."""
    found = gh_json("--paginate", "--slurp", f"repos/{repo}/actions/caches?per_page=100&ref={MAIN}&key={prefix}")
    keys: set[str] = set()
    for page in found or []:
        keys.update(c["key"] for c in page.get("actions_caches", []))
    return {k for k in keys if k.startswith(prefix)}


def check(repo: str, run_id: str, sccache: list[str], slice_jobs: list[str]) -> list[str]:
    run = gh_json(f"repos/{repo}/actions/runs/{run_id}")
    pages = gh_json("--paginate", "--slurp", f"repos/{repo}/actions/runs/{run_id}/jobs?per_page=100")
    jobs = [j for page in pages for j in page["jobs"]]
    days = {day(run["run_started_at"]), day(run["updated_at"])}
    problems: list[str] = []

    def ran(name_prefix: str) -> list[dict]:
        return [j for j in jobs if j["name"].startswith(name_prefix) and j["conclusion"] == "success"]

    for spec in sccache:
        name, _, prefix = spec.partition("=")
        if not ran(name):
            continue
        have = main_keys(repo, prefix)
        if not any(prefix + d in have for d in days):
            problems.append(f"{name}: main has no {prefix}<{'|'.join(sorted(days))}> entry after a green run")

    for name in slice_jobs:
        for job in ran(name):
            notes = gh_json(f"repos/{repo}/check-runs/{job['id']}/annotations?per_page=100") or []
            keys = [m.group(1) for n in notes if (m := NOTICE.search(n.get("message", "")))]
            if not keys:
                problems.append(f"{job['name']}: no 'SwiftUI Rust slice cache hit' notice, cannot find its key")
                continue
            for key in keys:
                if key not in main_keys(repo, key):
                    problems.append(f"{job['name']}: main does not hold the slice cache {key}")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--repo", required=True)
    ap.add_argument("--run-id", required=True)
    ap.add_argument("--sccache", action="append", default=[], metavar="JOB=PREFIX")
    ap.add_argument("--slice", action="append", default=[], dest="slice_jobs", metavar="JOB")
    args = ap.parse_args(argv)
    problems = check(args.repo, args.run_id, args.sccache, args.slice_jobs)
    for p in problems:
        print(f"::error::{p}")
    if not problems:
        print("main's build caches are saved")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
