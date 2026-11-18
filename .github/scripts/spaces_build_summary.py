#!/usr/bin/env python3
"""Cost summary of a workflow run (CUA-2066, phase 1 of CUA-1991).

Reads the run's jobs and artifacts through `gh api` and prints Markdown: wall
clock duration, runner minutes per OS (each job rounded up to a whole minute,
as GitHub bills) and artifact size. The caller appends it to
$GITHUB_STEP_SUMMARY. Jobs that have not finished (this one) are skipped.

    spaces_build_summary.py REPO RUN_ID [FINAL_ARTIFACT_NAME]
"""
import json
import math
import subprocess
import sys
from datetime import datetime

# List-price multipliers of GitHub-hosted runners, to compare cost across OSes.
WEIGHT = {"Linux": 1, "Windows": 2, "macOS": 10}


def gh(path: str):
    out = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", path], check=True, capture_output=True, text=True
    ).stdout
    return json.loads(out)


def ts(s: str) -> datetime:
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def os_of(job) -> str:
    text = " ".join(job.get("labels") or []) + " " + (job.get("runner_name") or "")
    text = text.lower()
    if "macos" in text:
        return "macOS"
    if "windows" in text:
        return "Windows"
    return "Linux"


def mb(n: int) -> str:
    return f"{n / 1e6:.1f} MB"


def main() -> int:
    repo, run_id = sys.argv[1], sys.argv[2]
    final = sys.argv[3] if len(sys.argv) > 3 else ""
    run = json.loads(
        subprocess.run(["gh", "api", f"repos/{repo}/actions/runs/{run_id}"], check=True, capture_output=True, text=True).stdout
    )
    jobs = [j for pages in gh(f"repos/{repo}/actions/runs/{run_id}/jobs?per_page=100") for j in pages["jobs"]]
    arts = [a for pages in gh(f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100") for a in pages["artifacts"]]

    minutes = {"Linux": 0, "Windows": 0, "macOS": 0}
    seconds = {"Linux": 0.0, "Windows": 0.0, "macOS": 0.0}
    rows = []
    for j in jobs:
        if not j.get("started_at") or not j.get("completed_at") or j.get("conclusion") == "skipped":
            continue
        secs = (ts(j["completed_at"]) - ts(j["started_at"])).total_seconds()
        o = os_of(j)
        minutes[o] += math.ceil(secs / 60)
        seconds[o] += secs
        rows.append((j["name"], o, secs, j.get("conclusion")))

    # Wall clock: from the run start to the latest finished job.
    ends = [ts(j["completed_at"]) for j in jobs if j.get("completed_at")]
    wall = (max(ends) - ts(run["run_started_at"])).total_seconds() if ends else 0
    total_art = sum(a["size_in_bytes"] for a in arts)
    final_art = sum(a["size_in_bytes"] for a in arts if a["name"] == final) if final else 0

    out = []
    out.append("## Build cost (CUA-1991 phase 1)")
    out.append("")
    out.append(f"- Wall clock so far: **{int(wall // 60)} min {int(wall % 60)} s**")
    out.append(f"- Final artifact `{final}`: **{mb(final_art)}**" if final else "- Final artifact: none")
    out.append(f"- All artifacts of the run (intermediate included): {mb(total_art)} in {len(arts)}")
    out.append("")
    out.append("| Runner OS | Runner minutes (rounded up per job) | List-price weight | Weighted minutes |")
    out.append("|---|---:|---:|---:|")
    for o in ("macOS", "Windows", "Linux"):
        out.append(f"| {o} | {minutes[o]} | x{WEIGHT[o]} | {minutes[o] * WEIGHT[o]} |")
    out.append(f"| Total | {sum(minutes.values())} | | {sum(m * WEIGHT[o] for o, m in minutes.items())} |")
    out.append("")
    out.append("| Job | OS | Duration |")
    out.append("|---|---|---:|")
    for name, o, secs, conclusion in sorted(rows, key=lambda r: -r[2]):
        out.append(f"| {name} ({conclusion}) | {o} | {int(secs // 60)} min {int(secs % 60)} s |")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
