#!/usr/bin/env python3
"""Reuse a green pull request run on the push to main that lands the same tree.

A squash merge pushes a commit to main. When no other commit reached main
between the pull request's last run and the merge, that commit's tree is
byte for byte the tree the pull request run already tested (the run checks out
GitHub's merge of the head onto main). Re-running those jobs on main spends
runner time and proves nothing new.

Two steps, both fail open (any doubt means "run everything"):

  stamp   On a pull_request run, record the tree that run tested as a notice
          annotation on one job: `ci-reuse tree=<40 hex>`.
  reuse   On a push to main, find the pull request behind the commit, find its
          successful runs of this same workflow, keep those whose stamp equals
          the pushed tree, and report the names of their successful jobs.

`run` does the right one for the event and writes step outputs:

  reused       JSON array of job names whose result can be reused ([] if none)
  <name>       "true" when every job named in `--subset name=a|b|c` was reused
               (all or nothing; "false" otherwise, and on every other event)

`legs` removes already-reused legs from a matrix list.

Only these jobs are skipped on main: ones that ran, with the same name, on a
green run of the same workflow for the identical tree. Legs the pull request
did not run (the macOS and Windows lanes) are never in the reused list, so they
still run on main. Push runs on any other branch, nightly runs and manual runs
are never reduced.

Modes (repository variable CI_REUSE_PR_RESULTS, passed as REUSE_MODE):
  unset or "on"  enforce
  "shadow"       decide and report, but reuse nothing
  "off"          do nothing (the rollback switch; also stops writing stamps)
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from typing import Callable

DEFAULT_BRANCH = "main"
STAMP_RE = re.compile(r"^ci-reuse tree=([0-9a-f]{40})$")
MAX_RUNS = 5

Api = Callable[[str], object]


def gh_api(path: str) -> object:
    out = subprocess.run(["gh", "api", path], check=True, capture_output=True, text=True, timeout=60).stdout
    return json.loads(out)


def gh_api_jobs(path: str) -> list[dict]:
    """All jobs of a run (the jobs endpoint pages at 100)."""
    out = subprocess.run(
        ["gh", "api", "--paginate", path, "--jq", ".jobs[]"],
        check=True, capture_output=True, text=True, timeout=120,
    ).stdout
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def tree_of(sha: str) -> str:
    return subprocess.run(
        ["git", "rev-parse", f"{sha}^{{tree}}"], check=True, capture_output=True, text=True
    ).stdout.strip()


def find_reusable(
    api: Api,
    jobs_of: Callable[[str], list[dict]],
    repo: str,
    sha: str,
    tree: str,
    run_id: str,
    stamp_job: str,
) -> tuple[list[str], str]:
    """Names of jobs that already passed on `tree`, and a one-line reason."""
    commit = api(f"repos/{repo}/commits/{sha}")
    parents = commit.get("parents", [])
    if len(parents) != 1:
        return [], f"{sha[:10]} is not a single-parent commit"
    prs = [
        pr
        for pr in api(f"repos/{repo}/commits/{sha}/pulls")
        if pr.get("merge_commit_sha") == sha and pr.get("merged_at") and pr["base"]["ref"] == DEFAULT_BRANCH
    ]
    if len(prs) != 1:
        return [], f"{sha[:10]} is not the squash commit of exactly one pull request"
    pr = prs[0]
    head = pr["head"]["sha"]
    workflow_id = api(f"repos/{repo}/actions/runs/{run_id}")["workflow_id"]
    runs = api(
        f"repos/{repo}/actions/workflows/{workflow_id}/runs"
        f"?head_sha={head}&event=pull_request&status=success&per_page=100"
    )["workflow_runs"]
    runs = sorted(runs, key=lambda r: r["created_at"], reverse=True)[:MAX_RUNS]
    if not runs:
        return [], f"PR #{pr['number']} has no successful run of this workflow on its head"
    reused: list[str] = []
    matched: list[str] = []
    for run in runs:
        jobs = jobs_of(f"repos/{repo}/actions/runs/{run['id']}/jobs?filter=latest&per_page=100")
        stamp = next((j for j in jobs if j["name"] == stamp_job), None)
        if stamp is None:
            continue
        notes = api(f"repos/{repo}/check-runs/{stamp['id']}/annotations")
        stamped = [m.group(1) for n in notes if (m := STAMP_RE.match(n.get("message") or ""))]
        if tree not in stamped:
            continue
        matched.append(str(run["id"]))
        for job in jobs:
            if job["conclusion"] == "success" and job["name"] not in reused:
                reused.append(job["name"])
    if not matched:
        return [], f"PR #{pr['number']} was last tested on a different tree (main moved or the head changed)"
    return reused, f"tree {tree[:10]} was tested green by PR #{pr['number']} run {', '.join(matched)}"


def decide(
    env: dict[str, str],
    subsets: dict[str, list[str]],
    stamp_job: str,
    api: Api = gh_api,
    jobs_of: Callable[[str], list[dict]] = gh_api_jobs,
    tree_fn: Callable[[str], str] = tree_of,
) -> tuple[list[str], dict[str, bool], str, str | None]:
    """Returns (reused, subset flags, reason, annotation to print or None)."""
    none = {name: False for name in subsets}
    mode = (env.get("REUSE_MODE") or "on").strip().lower()
    event = env.get("GITHUB_EVENT_NAME", "")
    sha = env.get("GITHUB_SHA", "")
    if mode == "off":
        return [], none, "disabled by CI_REUSE_PR_RESULTS=off", None
    try:
        if event == "pull_request":
            tree = tree_fn(sha)
            return [], none, f"stamped tree {tree[:10]}", f"::notice title=ci-reuse::ci-reuse tree={tree}"
        if event != "push" or env.get("GITHUB_REF") != f"refs/heads/{DEFAULT_BRANCH}":
            return [], none, f"{event or 'this event'} always runs everything", None
        reused, why = find_reusable(
            api, jobs_of, env["GITHUB_REPOSITORY"], sha, tree_fn(sha), env["GITHUB_RUN_ID"], stamp_job
        )
    except Exception as err:  # fail open: a lookup problem must never cost coverage
        return [], none, f"lookup failed, running everything ({type(err).__name__}: {err})", None
    flags = {name: bool(names) and all(n in reused for n in names) for name, names in subsets.items()}
    if mode == "shadow":
        return [], none, f"shadow: would reuse {len(reused)} jobs, {why}", None
    return reused, flags, why, None


def remaining_legs(legs: list[str], template: str, reused: list[str]) -> list[str]:
    return [leg for leg in legs if template.replace("{os}", leg) not in reused]


def write_outputs(pairs: dict[str, str]) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    for key, value in pairs.items():
        print(f"{key}={value}")
        if path:
            with open(path, "a") as fh:
                fh.write(f"{key}={value}\n")


def summary(line: str) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    print(line)
    if path:
        with open(path, "a") as fh:
            fh.write(line + "\n")


def parse_subset(arg: str) -> tuple[str, list[str]]:
    name, _, names = arg.partition("=")
    return name, [n for n in names.split("|") if n]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run")
    run.add_argument("--stamp-job", required=True, help="name of the job that carries the stamp annotation")
    run.add_argument("--subset", action="append", default=[], metavar="OUTPUT=job|job|...")
    legs = sub.add_parser("legs")
    legs.add_argument("--template", required=True, help="job name with {os}, e.g. 'rust ({os})'")
    legs.add_argument("--output", required=True)
    args = parser.parse_args(argv)

    if args.cmd == "run":
        subsets = dict(parse_subset(s) for s in args.subset)
        reused, flags, why, note = decide(dict(os.environ), subsets, args.stamp_job)
        if note:
            print(note)
        summary(f"CI reuse: {why}")
        if reused:
            summary("Reused on this tree: " + ", ".join(f"`{n}`" for n in reused))
        write_outputs({"reused": json.dumps(reused), **{k: str(v).lower() for k, v in flags.items()}})
        return 0

    reused = json.loads(os.environ.get("REUSED") or "[]")
    all_legs = json.loads(os.environ["LEGS"])
    left = remaining_legs(all_legs, args.template, reused)
    write_outputs({args.output: json.dumps(left)})
    if left != all_legs:
        summary(f"CI reuse: `{args.template}` skips {sorted(set(all_legs) - set(left))}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
