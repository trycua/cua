#!/usr/bin/env python3
"""Weekly CI speed report: opens or updates ONE issue, "CI speed this week" (CUA-3558, cut #12).

It measures what the CI audit of 5 Nov 2026 had to estimate by hand, so the next
round of cuts can be ranked from numbers:

  * push-to-green: from a PR's last push (its first check starting) to a green
    `ci-gate`, for PRs merged in the last 7 days;
  * the gate tail: how long `ci-gate` kept running after the last other check
    finished, split into "waiting for the local-ci sign-off" and "other";
  * merge wait: from a green `ci-gate` to the merge;
  * release-PR wait: release-please PRs, opened to merged and green to merged;
  * CD time: the median and p90 duration of every `CD:` workflow run;
  * the slowest test files, from the `timings-*` artifacts that CI jobs upload
    (.github/scripts/flaky/timings.py).

It follows the weekly flaky-test issue (.github/scripts/flaky/flaky.py digest):
one issue, found by a marker, rewritten each Monday by .github/workflows/ci-weekly-report.yml.

  ci_weekly_report.py [--days 7] [--dry-run]
"""

from __future__ import annotations

import argparse
import datetime as dt
import io
import json
import os
import statistics
import subprocess
import sys
import time
import zipfile
from collections import defaultdict
from pathlib import Path
from typing import Dict, Iterable, List, Optional

TITLE = "CI speed this week"
MARKER = "<!-- ci-speed-weekly -->"
LABEL = "ci-speed"
GATE = "ci-gate"
SIGNOFF = "local-ci"
RERUN_SLACK_MIN = 2.0  # a gate that starts this long after the last check is a re-run, not a wait

PR_QUERY = """
query($q: String!, $after: String) {
  search(query: $q, type: ISSUE, first: 25, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest {
      number createdAt mergedAt headRefName
      commits(last: 1) { nodes { commit { oid status { contexts { context createdAt } } } } }
    } }
  }
}
"""

# Workflow runs the gate never waits for (see ci-gate.yml); they must not count as "the last check".
GATE_IGNORES = ("claude-auto-fix", "ci-release-reminder", "review-cua-perception-candidates", "cla.yml")


def gh(*args: str, check: bool = True) -> str:
    for attempt in range(3):  # the API sometimes answers 5xx or an empty body
        done = subprocess.run(["gh", *args], capture_output=True, text=True)
        if done.returncode == 0:
            break
        time.sleep(5 * (attempt + 1))
    if done.returncode != 0 and check:
        raise RuntimeError(f"gh {' '.join(args[:3])} failed: {done.stderr.strip()[:300]}")
    return done.stdout


def ts(value: Optional[str]) -> Optional[dt.datetime]:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00")) if value else None


def minutes(a: dt.datetime, b: dt.datetime) -> float:
    return (b - a).total_seconds() / 60


def pct(values: List[float], p: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, max(0, int(round(p * len(ordered))) - 1))]


def summarize(values: List[float]) -> str:
    if not values:
        return "no data"
    return f"median {statistics.median(values):.1f} min, p90 {pct(values, 0.9):.1f} min (n={len(values)})"


# --- pull requests -----------------------------------------------------------


def pr_metrics(pr: dict, runs: List[dict]) -> Optional[dict]:
    """The timings of one merged PR. `runs` are the workflow runs of its last commit
    ({name, path, started, updated, conclusion}). None when it has no finished gate.

    The gate tail is a lower bound: a run the gate skipped by scope that finished late still counts as
    "the last check", which can only shorten it.
    """
    done = [r for r in runs if r.get("started") and r.get("updated") and r.get("conclusion")]
    gates = [r for r in done if r["name"] == "CI: gate" and r["conclusion"] == "success"]
    if not gates:
        return None
    gate = min(gates, key=lambda r: ts(r["updated"]))  # the first green; later ones are label or edit re-runs
    gate_start, gate_end = ts(gate["started"]), ts(gate["updated"])
    others = [r for r in done if r["name"] != "CI: gate" and not any(i in (r.get("path") or "") for i in GATE_IGNORES)
              and r["conclusion"] != "cancelled"]
    if not others:
        return None
    first = min(ts(r["started"]) for r in others)
    ended = [ts(r["updated"]) for r in others if ts(r["updated"]) <= gate_end]
    last = max(ended) if ended else gate_end
    rerun = minutes(last, gate_start) > RERUN_SLACK_MIN  # label or edit event, after everything finished
    commit = ((pr.get("commits") or {}).get("nodes") or [{}])[0].get("commit") or {}
    contexts = ((commit.get("status") or {}).get("contexts")) or []
    out = {
        "number": pr["number"],
        "release": (pr.get("headRefName") or "").startswith("release-please--"),
        "push_to_green": minutes(first, last if rerun else gate_end),
        "merge_wait": max(0.0, minutes(gate_end, ts(pr["mergedAt"]))),
        "opened_to_merged": minutes(ts(pr["createdAt"]), ts(pr["mergedAt"])),
        "tail": None,
        "tail_signoff": None,
    }
    if not rerun:
        tail = max(0.0, minutes(last, gate_end))
        signoff = next((ts(c["createdAt"]) for c in contexts if c.get("context") == SIGNOFF and c.get("createdAt")), None)
        out["tail"] = tail
        out["tail_signoff"] = bool(signoff and last <= signoff <= gate_end and tail > 3)
    return out


def fetch_runs(oid: str) -> List[dict]:
    repo = os.environ["GITHUB_REPOSITORY"]
    out = gh("api", f"repos/{repo}/actions/runs?head_sha={oid}&per_page=100", "--jq",
             '.workflow_runs[] | select(.event == "pull_request")'
             ' | {name, path, started: .run_started_at, updated: .updated_at, conclusion}')
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def fetch_prs(days: int) -> List[dict]:
    since = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days)).strftime("%Y-%m-%d")
    repo = os.environ["GITHUB_REPOSITORY"]
    q = f"repo:{repo} is:pr is:merged base:main merged:>={since}"
    out, after = [], None
    for _ in range(30):
        args = ["api", "graphql", "-f", f"query={PR_QUERY}", "-f", f"q={q}"]
        if after:
            args += ["-f", f"after={after}"]
        data = json.loads(gh(*args))["data"]["search"]
        out += [n for n in data["nodes"] if n]
        if not data["pageInfo"]["hasNextPage"]:
            break
        after = data["pageInfo"]["endCursor"]
    return out


# --- CD workflows ------------------------------------------------------------


def cd_durations(days: int) -> Dict[str, dict]:
    repo = os.environ["GITHUB_REPOSITORY"]
    since = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days)).strftime("%Y-%m-%d")
    lines = gh("api", f"repos/{repo}/actions/workflows?per_page=100", "--paginate", "--jq",
               '.workflows[] | select(.name | startswith("CD:")) | {id, name}')
    workflows = [json.loads(line) for line in lines.splitlines() if line.strip()]
    out: Dict[str, dict] = {}
    for wf in workflows:
        runs = json.loads(gh("api", f"repos/{repo}/actions/workflows/{wf['id']}/runs?status=completed"
                             f"&created=>={since}&per_page=100", "--jq", "[.workflow_runs[] | {c: .conclusion, s: .run_started_at, u: .updated_at}]"))
        durs = [minutes(ts(r["s"]), ts(r["u"])) for r in runs if r["c"] in ("success", "failure") and r["s"]]
        if durs:
            out[wf["name"]] = {"runs": len(durs), "fail": sum(1 for r in runs if r["c"] == "failure"), "durations": durs}
    return out


# --- test timings --------------------------------------------------------------


def timing_workflows() -> List[str]:
    """Workflow files that upload `timings-*` artifacts (directly or through flaky-upload)."""
    root = Path(__file__).resolve().parents[1] / "workflows"
    return sorted(p.name for p in root.glob("*.yml") if any(
        needle in p.read_text() for needle in ("ci-timings-upload", "flaky-upload")))


def fetch_timings(days: int, runs_per_workflow: int = 10) -> List[dict]:
    """The per-file timings of the latest main-branch runs of each workflow that uploads them.

    The repository-wide artifact listing (`actions/artifacts`) answers 500 on a repository this
    size, so this goes through each workflow's runs instead.
    """
    repo = os.environ["GITHUB_REPOSITORY"]
    since = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days)).strftime("%Y-%m-%d")
    reports: List[dict] = []
    for wf in timing_workflows():
        try:
            run_ids = gh("api", f"repos/{repo}/actions/workflows/{wf}/runs?branch=main&event=push&status=completed"
                         f"&created=>={since}&per_page={runs_per_workflow}", "--jq", ".workflow_runs[].id").split()
            for run_id in run_ids:
                arts = gh("api", f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100", "--jq",
                          '.artifacts[] | select((.name | startswith("timings-")) and (.expired | not)) | .id').split()
                for art in arts:
                    done = subprocess.run(["gh", "api", f"repos/{repo}/actions/artifacts/{art}/zip"], capture_output=True)
                    if done.returncode != 0:
                        continue
                    with zipfile.ZipFile(io.BytesIO(done.stdout)) as z:
                        for name in z.namelist():
                            if name.endswith(".json"):
                                try:
                                    reports.append(json.loads(z.read(name)))
                                except ValueError:
                                    pass
        except RuntimeError as exc:  # one workflow's failure must not lose the whole report
            print(f"warning: no timings from {wf}: {exc}", file=sys.stderr)
    return reports


def slowest_files(reports: Iterable[dict], top: int = 20) -> List[dict]:
    seen: Dict[tuple, List[float]] = defaultdict(list)
    for r in reports:
        for f in r.get("files", []):
            seen[(r.get("suite", ""), f["file"])].append(float(f["seconds"]))
    rows = [{"suite": s, "file": f, "median": statistics.median(v), "max": max(v), "runs": len(v)}
            for (s, f), v in seen.items()]
    return sorted(rows, key=lambda r: -r["median"])[:top]


# --- report -------------------------------------------------------------------


def build_report(metrics: List[dict], cd: Dict[str, dict], timings: List[dict], since: str) -> str:
    normal = [m for m in metrics if not m["release"]]
    release = [m for m in metrics if m["release"]]
    tails = [m["tail"] for m in normal if m["tail"] is not None]
    signoff_tails = [m["tail"] for m in normal if m["tail"] is not None and m["tail_signoff"]]
    other_tails = [m["tail"] for m in normal if m["tail"] is not None and not m["tail_signoff"]]
    out = [MARKER, f"Measured over the last 7 days (since {since}). Rewritten every Monday by `ci-weekly-report.yml`; "
           "numbers are for merged PRs into `main` and from the last commit's checks.", ""]
    out += ["## Pull requests", "",
            "| Measure | Result |", "|---|---|",
            f"| Push to green (first check starts to green `ci-gate`) | {summarize([m['push_to_green'] for m in normal])} |",
            f"| Green to merged | {summarize([m['merge_wait'] for m in normal])} |",
            f"| `ci-gate` after the last other check, all | {summarize(tails)} |",
            f"| ...of which waiting for the `local-ci` sign-off | {summarize(signoff_tails)} |",
            f"| ...of which other (poll interval, flake reruns) | {summarize(other_tails)} |", ""]
    out += ["## Release PRs (release-please)", "",
            "| Measure | Result |", "|---|---|",
            f"| Opened to merged | {summarize([m['opened_to_merged'] for m in release])} |",
            f"| Green to merged (the wait for a human) | {summarize([m['merge_wait'] for m in release])} |", ""]
    out += ["## CD time", "", "| Workflow | Runs | Failed | Median | p90 |", "|---|---:|---:|---:|---:|"]
    for name, d in sorted(cd.items(), key=lambda kv: -statistics.median(kv[1]["durations"])):
        out.append(f"| {name} | {d['runs']} | {d['fail']} | {statistics.median(d['durations']):.0f} min | {pct(d['durations'], 0.9):.0f} min |")
    if not cd:
        out.append("| no CD runs this week | | | | |")
    out += ["", "## Slowest test files", ""]
    rows = slowest_files(timings)
    if rows:
        out += ["| Suite | File | Median | Max | Runs |", "|---|---|---:|---:|---:|"]
        out += [f"| {r['suite']} | `{r['file']}` | {r['median']:.0f} s | {r['max']:.0f} s | {r['runs']} |" for r in rows]
    else:
        out.append("No `timings-*` artifacts yet (CI jobs upload them for 10 days).")
    return "\n".join(out) + "\n"


def collect_metrics(prs: List[dict]) -> List[dict]:
    out = []
    for pr in prs:
        commit = ((pr.get("commits") or {}).get("nodes") or [{}])[0].get("commit") or {}
        if not commit.get("oid"):
            continue
        try:
            m = pr_metrics(pr, fetch_runs(commit["oid"]))
        except RuntimeError as exc:
            print(f"warning: PR #{pr['number']}: {exc}", file=sys.stderr)
            continue
        if m:
            out.append(m)
    return out


def main(argv: Optional[List[str]] = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--days", type=int, default=7)
    p.add_argument("--dry-run", action="store_true")
    args = p.parse_args(argv)
    repo = os.environ["GITHUB_REPOSITORY"]
    since = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=args.days)).strftime("%Y-%m-%d")
    body = build_report(collect_metrics(fetch_prs(args.days)), cd_durations(args.days), fetch_timings(args.days), since)
    print(body)
    if args.dry_run:
        return 0
    gh("label", "create", LABEL, "--repo", repo, "--color", "0E8A16", "--description", "Weekly CI speed report", check=False)
    existing = json.loads(gh("issue", "list", "--repo", repo, "--label", LABEL, "--state", "open", "--limit", "50",
                             "--json", "number,body") or "[]")
    issue = next((i for i in existing if MARKER in (i.get("body") or "")), None)
    if issue:
        gh("issue", "edit", str(issue["number"]), "--repo", repo, "--body", body)
        print(f"updated #{issue['number']}")
    else:
        print(gh("issue", "create", "--repo", repo, "--label", LABEL, "--title", TITLE, "--body", body).strip())
    return 0


if __name__ == "__main__":
    sys.exit(main())
