#!/usr/bin/env python3
"""Cache cost audit for the Rust build caches (Rust CI jobs).

A cache earns its place only when the compile time it saves on a run exceeds
what the cache costs that run: the restore step on every run, plus the save
step on the runs that save. This reads recent workflow runs with `gh api` and
prints, per job that has a rust-cache step (Swatinem/rust-cache or
.github/actions/rust-cache):

  restore    median seconds of the cache step (restore, and sccache install)
  save       median seconds of the post step, over the runs that saved
  cold       the longest cold compile seen in the sampled logs: the sccache
             work that a warm cache would skip (executed compile requests times
             their average compile time, over 3.4 effective cores) when the log
             has sccache statistics from a run that mostly missed, otherwise the
             sum of cargo's "Finished ... in" lines
  benefit    estimated seconds a warm cache saves: 85% of the sccache figure
             (e2b-bindings: 745 predicted, 888 measured, from 32.8 to 18.0
             minutes with 0 of 2232 compile requests hitting cold and 99.9%
             warm), or 55% x 85% of the cargo wall time when there are no
             sccache statistics. The cargo figure runs high when a job's cargo
             invocations overlap (22 minutes predicted for the same job).
  verdict    keep when benefit >= 2 x cost and >= 45 s, otherwise drop?; "insufficient
             cold evidence" when every sampled log came from a run that hit a
             warm cache (its compile times are warm times, not what the cache
             saves): sample older runs or run the job cold before judging it

The audit cannot see whether anything saves the family it restores: a workflow
that runs only on tags or pull requests, with a family no main run saves, never
hits whatever its compile time (check the triggers). Output a job discards
(cargo's stderr, a script that runs cargo quietly) shows as a cold compile of
0 seconds: measure such a job from its sccache statistics after the change.

A restore grows with the size of the family's directory (about 25 s per GB,
measured on a macOS runner), so a short job that shares a multi-GB family pays
more than its own compile could save: give it a small family of its own or
drop the cache.

Usage (needs the `gh` CLI, authenticated; run from any directory):

  ci_cache_audit.py --repo trycua/cua                       # every workflow with a rust-cache step
  ci_cache_audit.py --workflows ci-elastic.yml e2b-bindings.yml --runs 20
  ci_cache_audit.py --format json > audit.json

Numbers come from the runs the API still returns for each workflow (the last
`--runs` completed runs). Jobs with fewer than --min-runs samples are marked
"few runs". The estimate is a screening tool: confirm a "drop?" with the
job's own before and after timings. Cargo runs inside a Docker container (the
reference-image builds) are counted in "cold" although a host cache cannot help
them; check the step before keeping a cache on that evidence.
"""
from __future__ import annotations

import argparse
import concurrent.futures as futures
import datetime as dt
import json
import re
import statistics
import subprocess
import sys
import time
from collections import defaultdict
from pathlib import Path

ANSI = re.compile(r"\x1b\[[0-9;]*m")
CARGO_FINISHED = re.compile(r"Finished .* in (?:(\d+)m )?(\d+(?:\.\d+)?)s")
SCCACHE_EXECUTED = re.compile(r"^\S*\s*Compile requests executed\s+(\d+)", re.M)
SCCACHE_HITS = re.compile(r"^\S*\s*Cache hits\s+(\d+)\s*$", re.M)
SCCACHE_AVERAGE = re.compile(r"^\S*\s*Average compiler\s+([\d.]+) s", re.M)
EFFECTIVE_CORES = 3.4
CACHE_STEP = re.compile(r"rust-cache")
SAVED_FRACTION = 0.55
HIT_RATE = 0.85
MIN_BENEFIT_S = 45.0
MIN_RATIO = 2.0
SAVE_STEP_MIN_S = 8.0


def gh_api(path: str, raw: bool = False):
    args = ["gh", "api"] + (["--allow-escape-sequences"] if raw else []) + [path]
    for attempt in range(2):
        result = subprocess.run(args, capture_output=True, text=True)
        if not result.returncode:
            return result.stdout if raw else json.loads(result.stdout)
        time.sleep(5)
    print(f"warning: gh api {path} failed: {result.stderr.strip()[:200]}", file=sys.stderr)
    return None


def seconds(start: str, end: str) -> float:
    parse = lambda t: dt.datetime.fromisoformat(t.replace("Z", "+00:00"))
    return (parse(end) - parse(start)).total_seconds()


def cargo_seconds(log: str) -> float:
    """Sum of the compile wall times cargo printed in a job log."""
    total = 0.0
    for minutes, secs in CARGO_FINISHED.findall(ANSI.sub("", log)):
        total += int(minutes or 0) * 60 + float(secs)
    return total


def log_profile(log: str) -> dict:
    """What one job log says about cold compile time.

    cargo        sum of cargo's compile wall times in the log
    sccache_cold wall seconds of compile work a warm cache would skip (executed
                 requests times their average compile time), only from a run
                 that mostly missed
    warm         the run hit a warm cache (sccache hit rate above 50%, or a
                 Swatinem restore), so its timings are warm timings"""
    text = ANSI.sub("", log)
    profile = {"cargo": cargo_seconds(text), "sccache_cold": None, "warm": False}
    executed, hits, average = (pattern.findall(text) for pattern in (SCCACHE_EXECUTED, SCCACHE_HITS, SCCACHE_AVERAGE))
    if executed and hits and average and int(executed[-1]):
        if int(hits[-1]) / int(executed[-1]) > 0.5:
            profile["warm"] = True
        else:
            profile["sccache_cold"] = int(executed[-1]) * float(average[-1]) / EFFECTIVE_CORES
    elif "Cache restored from key" in text:
        profile["warm"] = True
    return profile


def sccache_cold_seconds(log: str) -> float | None:
    return log_profile(log)["sccache_cold"]


def combine(profiles: list[dict]) -> tuple[float, str]:
    """Cold compile seconds over the sampled logs, and the basis: "sccache"
    (statistics from a run that mostly missed), "cargo" (cargo timings of runs
    that did not hit a cache) or "warm-only" (every sampled run was warm, so
    there is no cold evidence)."""
    cold = [p["sccache_cold"] for p in profiles if p["sccache_cold"]]
    if cold:
        return max(cold), "sccache"
    unwarmed = [p["cargo"] for p in profiles if not p["warm"]]
    if unwarmed:
        return max(unwarmed), "cargo"
    return 0.0, "warm-only" if profiles else "cargo"


def estimate(cold_s: float, from_sccache: bool = False) -> float:
    """Seconds saved by a warm cache."""
    return HIT_RATE * cold_s if from_sccache else SAVED_FRACTION * HIT_RATE * cold_s


def verdict(benefit_s: float, restore_s: float, save_s: float, save_share: float, samples: int, min_runs: int,
            basis: str = "cargo") -> str:
    cost = restore_s + save_s * save_share
    if samples < min_runs:
        return "few runs"
    if basis == "warm-only":
        return "insufficient cold evidence"
    if benefit_s >= MIN_BENEFIT_S and benefit_s >= MIN_RATIO * cost:
        return "keep"
    return "drop?"


def workflows_with_cache(root: Path) -> list[str]:
    found = []
    for path in sorted((root / ".github" / "workflows").glob("*.yml")):
        text = path.read_text(encoding="utf-8", errors="replace")
        if "Swatinem/rust-cache" in text or "actions/rust-cache" in text:
            found.append(path.name)
    return found


def collect_jobs(repo: str, workflow: str, runs: int) -> list[dict]:
    listing = gh_api(f"repos/{repo}/actions/workflows/{workflow}/runs?per_page=50&status=completed")
    if not listing:
        return []
    rows = []
    done = [r for r in listing["workflow_runs"] if r["conclusion"] in ("success", "failure")][:runs]
    for run in done:
        jobs = gh_api(f"repos/{repo}/actions/runs/{run['id']}/jobs?per_page=100")
        for job in (jobs or {}).get("jobs", []):
            if job["conclusion"] not in ("success", "failure") or not job.get("completed_at"):
                continue
            cache = [s for s in job["steps"] if CACHE_STEP.search(s["name"]) and s.get("completed_at")]
            if not cache:
                continue
            restore = sum(seconds(s["started_at"], s["completed_at"]) for s in cache if not s["name"].startswith("Post "))
            save = sum(seconds(s["started_at"], s["completed_at"]) for s in cache if s["name"].startswith("Post "))
            rows.append({
                "workflow": workflow, "job": job["name"], "job_id": job["id"], "run": run["id"],
                "event": run["event"], "restore": restore, "save": save,
                "total": seconds(job["started_at"], job["completed_at"]),
            })
    return rows


def cold_compile(repo: str, samples: list[dict]) -> tuple[float, str]:
    """Cold compile seconds and their basis, from the logs of the shortest,
    median and longest job of the group."""
    ordered = sorted(samples, key=lambda x: x["total"])
    picks = {job["job_id"]: job for job in (ordered[0], ordered[len(ordered) // 2], ordered[-1])}
    profiles = []
    for job in picks.values():
        log = gh_api(f"repos/{repo}/actions/jobs/{job['job_id']}/logs", raw=True)
        if log:
            profiles.append(log_profile(log))
    return combine(profiles)


def audit(repo: str, workflows: list[str], runs: int, min_runs: int, workers: int) -> list[dict]:
    with futures.ThreadPoolExecutor(workers) as pool:
        collected = list(pool.map(lambda w: collect_jobs(repo, w, runs), workflows))
    groups: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for rows in collected:
        for row in rows:
            groups[(row["workflow"], row["job"])].append(row)
    with futures.ThreadPoolExecutor(workers) as pool:
        keys = list(groups)
        colds = list(pool.map(lambda k: cold_compile(repo, groups[k]), keys))
    report = []
    for key, (cold, basis) in zip(keys, colds):
        samples = groups[key]
        saves = [x["save"] for x in samples if x["save"] > SAVE_STEP_MIN_S]
        restore = statistics.median(x["restore"] for x in samples)
        save = statistics.median(saves) if saves else 0.0
        share = len(saves) / len(samples)
        benefit = estimate(cold, basis == "sccache")
        report.append({
            "workflow": key[0], "job": key[1], "runs": len(samples),
            "restore_s": round(restore), "save_s": round(save), "save_share": round(share, 2),
            "cold_compile_s": round(cold), "basis": basis, "benefit_s": round(benefit),
            "verdict": verdict(benefit, restore, save, share, len(samples), min_runs, basis),
        })
    order = {"drop?": 0, "insufficient cold evidence": 1}
    return sorted(report, key=lambda r: (order.get(r["verdict"], 2), r["workflow"], r["job"]))


def markdown(report: list[dict]) -> str:
    lines = ["| workflow | job | runs | restore s | save s (share) | cold compile s (basis) | est. benefit s | verdict |",
             "|---|---|---|---|---|---|---|---|"]
    for r in report:
        lines.append(f"| {r['workflow']} | {r['job']} | {r['runs']} | {r['restore_s']} | {r['save_s']} ({r['save_share']}) | "
                     f"{r['cold_compile_s']} ({r['basis']}) | {r['benefit_s']} | {r['verdict']} |")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", default="trycua/cua")
    parser.add_argument("--workflows", nargs="*", help="workflow file names (default: every workflow with a rust-cache step)")
    parser.add_argument("--runs", type=int, default=12, help="completed runs sampled per workflow")
    parser.add_argument("--min-runs", type=int, default=3)
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--format", choices=("md", "json"), default="md")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2], help="checkout used to find workflows")
    args = parser.parse_args(argv)
    workflows = args.workflows or workflows_with_cache(args.root)
    report = audit(args.repo, workflows, args.runs, args.min_runs, args.workers)
    print(json.dumps(report, indent=2) if args.format == "json" else markdown(report))
    return 0


if __name__ == "__main__":
    sys.exit(main())
