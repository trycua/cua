#!/usr/bin/env python3
"""Is main's Spaces CI green at this commit? The gate of nightly-cua-spaces.yml.

The nightly builds main's head only when the Spaces CI workflows
(.github/workflows/ci-cua-spaces*.yml, which run on every push to main that
touches their paths) passed on it. A commit that ran none of them (it changed
nothing Spaces reads) has the CI state of the newest first-parent ancestor
that ran them, so the walk goes back until it finds one.

  python3 spaces_nightly_gate.py OWNER/REPO SHA [--wait-minutes N]

Prints `green`, `red` or `pending` (pending only once the wait ran out) and,
for red, the failed workflows on stderr. Exit code 0 in every case: the
caller decides. Needs GH_TOKEN and a git checkout with the history.

`cancelled` runs do not count: main's CI cancels a run a newer push
supersedes, and that says nothing about the code.
"""
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

SPACES_CI = re.compile(r"^\.github/workflows/ci-cua-spaces[\w.-]*\.ya?ml$")
RED = {"failure", "timed_out", "startup_failure", "action_required"}
# How far back a commit's CI state is looked for before the gate gives up.
MAX_WALK = 60


def spaces_runs(runs):
    """The push runs of the Spaces CI workflows, the newest of each (a re-run keeps its id)."""
    newest = {}
    for run in runs:
        if run.get("event") != "push" or not SPACES_CI.match(run.get("path") or ""):
            continue
        if run["path"] not in newest or run["id"] > newest[run["path"]]["id"]:
            newest[run["path"]] = run
    return list(newest.values())


def verdict(runs):
    """`green`, `red`, `pending` or `none` (no Spaces CI ran) for one commit's runs."""
    mine = spaces_runs(runs)
    if not mine:
        return "none", []
    failed = sorted(r["path"].rsplit("/", 1)[-1] for r in mine if r.get("status") == "completed" and r.get("conclusion") in RED)
    if failed:
        return "red", failed
    if any(r.get("status") != "completed" for r in mine):
        return "pending", []
    return "green", []


def _runs_for(repo, sha):
    req = urllib.request.Request(
        f"https://api.github.com/repos/{repo}/actions/runs?head_sha={sha}&event=push&per_page=100",
        headers={"Authorization": f"Bearer {os.environ['GH_TOKEN']}", "Accept": "application/vnd.github+json"},
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        return json.load(resp)["workflow_runs"]


def commit_state(repo, sha, fetch=_runs_for, parents=None):
    """The Spaces CI verdict of `sha`, from the newest first-parent ancestor that ran it."""
    if parents is None:
        parents = subprocess.run(
            ["git", "rev-list", "--first-parent", f"--max-count={MAX_WALK}", sha],
            check=True, capture_output=True, text=True,
        ).stdout.split()
    for commit in parents:
        state, failed = verdict(fetch(repo, commit))
        if state != "none":
            return state, failed, commit
    return "pending", [], None


def main(argv):
    args = argv[1:]
    wait = 0
    if "--wait-minutes" in args:
        i = args.index("--wait-minutes")
        wait = int(args[i + 1])
        del args[i:i + 2]
    if len(args) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    repo, sha = args
    deadline = time.monotonic() + wait * 60
    while True:
        state, failed, at = commit_state(repo, sha)
        if state != "pending" or time.monotonic() >= deadline:
            break
        time.sleep(60)
    if at and at != sha:
        print(f"Spaces CI state taken from {at[:7]} (the newest commit that ran it)", file=sys.stderr)
    for name in failed:
        print(f"failed: {name}", file=sys.stderr)
    print(state)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
