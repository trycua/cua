# SPDX-License-Identifier: FSL-1.1-Apache-2.0
"""Keep the repository's GitHub Actions cache budget for main's caches.

Main's Rust build caches (sccache, one entry per family per UTC day) and the
nix caches are what make pull requests fast. They are evicted when the
repository's budget fills, so this tool removes what nothing can use:

* caches of pull requests that are closed or merged (a PR entry is readable
  only by that PR);
* caches of tags older than TAG_DAYS (release builds; readable only by their
  own tag);
* caches of other branches older than BRANCH_DAYS (manual runs that measure
  a cache change);
* superseded sccache and nix entries on main: for each cache family (the key
  without its trailing UTC date or content hash) only the newest KEEP entries stay.

It never deletes a cache whose ref is main except to supersede it with a
newer entry of the same family. Run it with --dry-run to see the plan.
"""

import argparse
import json
import os
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone

TAG_DAYS = 2
BRANCH_DAYS = 2
KEEP = 1
# Families whose newer entry replaces the older one outright. Other keys (node
# package caches) are hashes of different lockfiles, not versions of one cache.
SUPERSEDE_PREFIXES = ("sccache-v2-", "nix-")

PR_REF = re.compile(r"^refs/pull/(\d+)/(merge|head)$")


def family(key):
    """The key without its trailing UTC date (sccache) or content hash (nix, rust)."""
    key = re.sub(r"_\d{8}$", "", key)
    return re.sub(r"[-_][0-9a-f]{16,}$", "", key)


def parse_time(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def plan(caches, open_prs, now, main_ref="refs/heads/main"):
    """Return [(cache, reason)] to delete. `open_prs` is the set of open PR numbers."""
    doomed = {}
    by_family = {}
    for c in caches:
        ref = c["ref"]
        age = now - parse_time(c["created_at"])
        m = PR_REF.match(ref)
        if m:
            if int(m.group(1)) not in open_prs:
                doomed[c["id"]] = (c, "closed pull request")
        elif "refs/tags/" in ref:  # also "refs/heads/refs/tags/x" (dispatch on a tag name)
            if age > timedelta(days=TAG_DAYS):
                doomed[c["id"]] = (c, "old tag")
        elif ref != main_ref:
            if age > timedelta(days=BRANCH_DAYS):
                doomed[c["id"]] = (c, "old branch")
        elif c["key"].startswith(SUPERSEDE_PREFIXES):
            by_family.setdefault(family(c["key"]), []).append(c)
    for fam, entries in by_family.items():
        entries.sort(key=lambda c: parse_time(c["created_at"]), reverse=True)
        for c in entries[KEEP:]:
            doomed[c["id"]] = (c, "superseded on main")
    return list(doomed.values())


def gh(*args):
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def list_caches(repo):
    out = gh("api", "--paginate", f"repos/{repo}/actions/caches?per_page=100",
             "--jq", ".actions_caches[]")
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def open_pr_numbers(repo):
    out = gh("api", "--paginate", f"repos/{repo}/pulls?state=open&per_page=100", "--jq", ".[].number")
    return {int(x) for x in out.split()}


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "trycua/cua"))
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--only-pr", type=int, help="delete only the caches of this closed pull request")
    args = ap.parse_args(argv)

    caches = list_caches(args.repo)
    total = sum(c["size_in_bytes"] for c in caches)
    print(f"{len(caches)} caches, {total / 1e9:.2f} GB before")
    now = datetime.now(timezone.utc)
    if args.only_pr is not None:
        todo = [(c, "closed pull request") for c in caches
                if c["ref"] in (f"refs/pull/{args.only_pr}/merge", f"refs/pull/{args.only_pr}/head")]
    else:
        todo = plan(caches, open_pr_numbers(args.repo), now)
    freed = 0
    for c, why in todo:
        print(f"{'would delete' if args.dry_run else 'delete'} {c['id']} {c['size_in_bytes'] / 1e6:.0f} MB {c['ref']} {c['key'][:80]} ({why})")
        if not args.dry_run:
            gh("cache", "delete", str(c["id"]), "-R", args.repo)
        freed += c["size_in_bytes"]
    print(f"{len(todo)} caches, {freed / 1e9:.2f} GB {'would be ' if args.dry_run else ''}freed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
