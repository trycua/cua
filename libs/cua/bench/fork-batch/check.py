#!/usr/bin/env python3
"""Perf guard for batched forks: compares a `bench.py` result with the
committed baseline of its profile and fails on a regression.

    python3 check.py --profile linux-kvm-gh results/run.json

Fails (exit 1) when any fork failed, when a group was split while the
baseline's was not, when a machine started more forks at once than allowed,
or when `fork_ms_p50`, `fork_ms_p95`, `first_ready_ms` or `total_ms` is more
than `factor` (default 1.5) times the baseline. `--update` writes the result
as the profile's new baseline instead (after a deliberate change).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

BASELINE = Path(__file__).with_name("baseline.json")
TIMED = ("fork_ms_p50", "fork_ms_p95", "first_ready_ms", "total_ms")


def check(res: dict, base: dict, factor: float) -> list[str]:
    problems = []
    if res["failed"]:
        problems.append(f"{res['failed']} fork(s) failed: {res.get('failures')}")
    if res["ok"] != base["ok"]:
        problems.append(f"{res['ok']} ready forks, baseline {base['ok']}")
    if res.get("groups_split") and not base.get("groups_split"):
        problems.append(f"groups split over machines: {res['groups_split']}")
    allowed = res.get("parallel") or base.get("parallel")
    for host, peak in (res.get("peak_parallel") or {}).items():
        if allowed and peak > allowed:
            problems.append(f"{host} started {peak} forks at once (max {allowed})")
    for key in TIMED:
        got, want = res.get(key), base.get(key)
        if got is None or want is None:
            continue
        if got > factor * want:
            problems.append(f"{key} {got} ms is more than {factor}x the baseline {want} ms")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--profile", required=True)
    ap.add_argument("--factor", type=float, default=None)
    ap.add_argument("--update", action="store_true")
    ap.add_argument("result")
    args = ap.parse_args()
    res = json.loads(Path(args.result).read_text())
    if args.update and not BASELINE.exists():
        BASELINE.write_text(json.dumps({"factor": 1.5, "profiles": {}}) + "\n")
    baseline = json.loads(BASELINE.read_text())
    if args.update:
        keep = ("on", "count", "group_size", "parallel", "ok", "groups_split", *TIMED, "per_host", "peak_parallel")
        prev = baseline["profiles"].get(args.profile, {})
        baseline["profiles"][args.profile] = {k: res.get(k) for k in keep}
        if "factor" in prev:  # a per-profile tolerance survives a re-baseline
            baseline["profiles"][args.profile]["factor"] = prev["factor"]
        BASELINE.write_text(json.dumps(baseline, indent=2) + "\n")
        print(f"baseline {args.profile} updated")
        return 0
    base = baseline["profiles"].get(args.profile)
    if base is None:
        print(f"no baseline for {args.profile} in {BASELINE}", file=sys.stderr)
        return 2
    factor = args.factor or base.get("factor") or baseline.get("factor", 1.5)
    problems = check(res, base, factor)
    for key in TIMED:
        print(f"{key:16} {res.get(key)!s:>8} (baseline {base.get(key)})")
    if problems:
        for p in problems:
            print(f"REGRESSION: {p}", file=sys.stderr)
        return 1
    print(f"ok: within {factor}x of {args.profile}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
