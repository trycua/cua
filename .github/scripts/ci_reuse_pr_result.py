#!/usr/bin/env python3
"""Reuse a green pull request run on the push to main that lands the same tree.

A squash merge pushes a commit to main. When no other commit reached main
between the pull request's last run and the merge, that commit's tree is
byte for byte the tree the pull request run already tested (the run checks out
GitHub's merge of the head onto main). Re-running those jobs on main spends
runner time and proves nothing new.

The same tree is not the same inputs. A job also reads what floats outside the
repository: the runner image, `rust-toolchain@stable`, `node-version: "22"`,
the newest uv. So the stamp carries a fingerprint of those, and a run is reused
only when the fingerprint still matches and the run is recent (see MAX_AGE:
unpinned pip, npm and apt installs cannot be fingerprinted).

Two steps, both fail open (any doubt means "run everything"):

  stamp   On a pull_request run, record the tree that run tested and the
          fingerprint of its floating inputs as a notice annotation on one job:
          `ci-reuse tree=<40 hex> fp=<16 hex>`.
  reuse   On a push to main, find the pull request behind the commit, find its
          successful runs of this same workflow, keep those whose stamp equals
          the pushed tree and today's fingerprint, and report the names of their
          successful jobs.

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

The whole `run` step has a time budget (BUDGET_SECONDS). Every network call
takes what is left of it; when it is spent the step answers "run everything".
Workflows also give the step a hard `timeout-minutes: 1`.

Modes (repository variable CI_REUSE_PR_RESULTS, passed as REUSE_MODE). Only
these values are valid; anything else is treated as "off" (no reuse, no stamp):
  unset or empty  enforce
  "on"            enforce
  "shadow"        decide and report, but reuse nothing
  "off"           do nothing (the rollback switch; also stops writing stamps)
Case and surrounding spaces are ignored; a value that is only spaces is invalid.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timedelta, timezone
from typing import Callable

DEFAULT_BRANCH = "main"
STAMP_RE = re.compile(r"^ci-reuse tree=([0-9a-f]{40}) fp=([0-9a-f]{16}|none)$")
MAX_RUNS = 5
# A run older than this is never reused. Pull requests merge in about 75 minutes
# (p90 200); the cap bounds drift in inputs no fingerprint covers (pip, npm and
# apt installs without pins, macOS and Windows runner images).
MAX_AGE = timedelta(hours=6)
BUDGET_SECONDS = 45
CALL_SECONDS = 20
MODES = ("on", "shadow", "off")

Api = Callable[[str], object]

_deadline: float | None = None


def set_budget(seconds: float | None) -> None:
    global _deadline
    _deadline = None if seconds is None else time.monotonic() + seconds


def time_left() -> float:
    """Seconds one call may take; raises once the budget is spent."""
    if _deadline is None:
        return CALL_SECONDS
    left = _deadline - time.monotonic()
    if left <= 0.5:
        raise TimeoutError("time budget spent")
    return min(CALL_SECONDS, left)


def gh_api(path: str) -> object:
    out = subprocess.run(
        ["gh", "api", path], check=True, capture_output=True, text=True, timeout=time_left()
    ).stdout
    return json.loads(out)


def gh_api_jobs(path: str) -> list[dict]:
    """All jobs of a run (the jobs endpoint pages at 100)."""
    out = subprocess.run(
        ["gh", "api", "--paginate", path, "--jq", ".jobs[]"],
        check=True, capture_output=True, text=True, timeout=time_left(),
    ).stdout
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def tree_of(sha: str) -> str:
    return subprocess.run(
        ["git", "rev-parse", f"{sha}^{{tree}}"], check=True, capture_output=True, text=True, timeout=time_left()
    ).stdout.strip()


# ---- fingerprint of the inputs that float outside the repository -----------------


def http_text(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "cua-ci-reuse"})
    with urllib.request.urlopen(req, timeout=time_left()) as resp:
        return resp.read().decode()


def _rust_stable() -> str:
    # The hash of the manifest rustup resolves `stable` to: it changes on every release.
    return http_text("https://static.rust-lang.org/dist/channel-rust-stable.toml.sha256").split()[0]


def _node_22() -> str:
    return re.search(r"node-v(22\.\d+\.\d+)", http_text("https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt")).group(1)


def _python_312() -> str:
    versions = re.findall(r'href="(3\.12\.\d+)/"', http_text("https://www.python.org/ftp/python/"))
    return max(versions, key=lambda v: tuple(int(p) for p in v.split(".")))


def _latest_release(repo: str) -> str:
    return gh_api(f"repos/{repo}/releases/latest")["tag_name"]


# Name -> how to resolve what the floating spec means today. A workflow lists
# the ones its jobs use with `--float`.
FLOATS: dict[str, Callable[[], str]] = {
    "rust-stable": _rust_stable,  # dtolnay/rust-toolchain@stable
    "node-22": _node_22,  # actions/setup-node with node-version "22"
    "python-3.12": _python_312,  # actions/setup-python with python-version "3.12"
    "uv": lambda: _latest_release("astral-sh/uv"),  # astral-sh/setup-uv (latest)
    "python-standalone": lambda: _latest_release("astral-sh/python-build-standalone"),  # uv's managed Python
}


def fingerprint(env: dict[str, str], floats: list[str], resolvers: dict[str, Callable[[], str]] | None = None) -> str:
    """16 hex chars, or "none" when any input cannot be resolved (never matches)."""
    resolvers = FLOATS if resolvers is None else resolvers
    try:
        if not env.get("ImageVersion"):
            return "none"
        parts = {"image": f"{env.get('ImageOS', '')}/{env['ImageVersion']}"}
        for name in sorted(floats):
            parts[name] = resolvers[name]()
        return hashlib.sha256(json.dumps(parts, sort_keys=True).encode()).hexdigest()[:16]
    except Exception:
        return "none"


# ---- decision --------------------------------------------------------------------


def parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def find_reusable(
    api: Api,
    jobs_of: Callable[[str], list[dict]],
    repo: str,
    sha: str,
    tree: str,
    run_id: str,
    stamp_job: str,
    fp_fn: Callable[[], str],
    now: datetime,
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
    stamps = same_tree = too_old = 0
    current_fp: str | None = None
    for run in runs:
        jobs = jobs_of(f"repos/{repo}/actions/runs/{run['id']}/jobs?filter=latest&per_page=100")
        stamp = next((j for j in jobs if j["name"] == stamp_job), None)
        if stamp is None:
            continue
        notes = api(f"repos/{repo}/check-runs/{stamp['id']}/annotations")
        found = [m.groups() for n in notes if (m := STAMP_RE.match(n.get("message") or ""))]
        stamps += len(found)
        for stamped_tree, stamped_fp in found:
            if stamped_tree != tree:
                continue
            same_tree += 1
            if now - parse_time(run["created_at"]) > MAX_AGE:
                too_old += 1
                continue
            if current_fp is None:
                current_fp = fp_fn()
            if "none" in (stamped_fp, current_fp) or stamped_fp != current_fp:
                continue
            matched.append(str(run["id"]))
            for job in jobs:
                if job["conclusion"] == "success" and job["name"] not in reused:
                    reused.append(job["name"])
            break
    if matched:
        return reused, f"tree {tree[:10]} was tested green by PR #{pr['number']} run {', '.join(matched)}"
    who = f"PR #{pr['number']}"
    if not stamps:
        return [], f"{who} runs carry no tree stamp"
    if not same_tree:
        return [], f"{who} was last tested on a different tree (main moved or the head changed)"
    if too_old == same_tree:
        return [], f"{who} tested this tree more than {MAX_AGE.total_seconds() / 3600:g} hours ago"
    return [], f"{who} tested this tree, but the runner image or a floating tool version changed since"


def parse_mode(raw: str | None) -> str | None:
    """The mode, or None for a value that is not valid."""
    if raw is None or raw == "":
        return "on"
    mode = raw.strip().lower()
    return mode if mode in MODES else None


def decide(
    env: dict[str, str],
    subsets: dict[str, list[str]],
    stamp_job: str,
    floats: list[str] | None = None,
    api: Api = gh_api,
    jobs_of: Callable[[str], list[dict]] = gh_api_jobs,
    tree_fn: Callable[[str], str] = tree_of,
    fp_fn: Callable[[], str] | None = None,
    now: datetime | None = None,
) -> tuple[list[str], dict[str, bool], str, str | None]:
    """Returns (reused, subset flags, reason, annotation to print or None)."""
    none = {name: False for name in subsets}
    raw = env.get("REUSE_MODE")
    mode = parse_mode(raw)
    event = env.get("GITHUB_EVENT_NAME", "")
    sha = env.get("GITHUB_SHA", "")
    if mode is None:
        return [], none, f"CI_REUSE_PR_RESULTS={raw!r} is not one of {', '.join(MODES)}: reuse is off", None
    if mode == "off":
        return [], none, "disabled by CI_REUSE_PR_RESULTS=off", None
    fp_fn = fp_fn or (lambda: fingerprint(env, floats or []))
    try:
        if event == "pull_request":
            tree = tree_fn(sha)
            fp = fp_fn()
            return [], none, f"stamped tree {tree[:10]} fp {fp}", f"::notice title=ci-reuse::ci-reuse tree={tree} fp={fp}"
        if event != "push" or env.get("GITHUB_REF") != f"refs/heads/{DEFAULT_BRANCH}":
            return [], none, f"{event or 'this event'} always runs everything", None
        reused, why = find_reusable(
            api, jobs_of, env["GITHUB_REPOSITORY"], sha, tree_fn(sha), env["GITHUB_RUN_ID"], stamp_job,
            fp_fn, now or datetime.now(timezone.utc),
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
    run.add_argument("--float", action="append", default=[], choices=sorted(FLOATS), dest="floats",
                     help="a floating tool spec the workflow's jobs install (repeatable)")
    run.add_argument("--budget", type=float, default=BUDGET_SECONDS, help="seconds for every lookup together")
    legs = sub.add_parser("legs")
    legs.add_argument("--template", required=True, help="job name with {os}, e.g. 'rust ({os})'")
    legs.add_argument("--output", required=True)
    args = parser.parse_args(argv)

    if args.cmd == "run":
        set_budget(args.budget)
        subsets = dict(parse_subset(s) for s in args.subset)
        reused, flags, why, note = decide(dict(os.environ), subsets, args.stamp_job, args.floats)
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
