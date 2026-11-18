#!/usr/bin/env python3
"""Checks a fork fan-out run (run.sh's JSON) against the committed baseline.

    python3 check.py run.json baseline.json --host macos-hvf

Every metric is lower-is-better. A metric fails when it is above
`factor` (default 1.5; a host class may set its own) times its baseline, and above the baseline plus the
metric's absolute slack (so near-zero baselines do not fail on noise).
Waves are matched by fork count; a wave in the baseline that the run did not
measure fails, a wave the baseline does not know is reported only. Exits 1 on
a regression, 2 on a usage error.
"""

from __future__ import annotations

import argparse
import json
import sys

# Absolute slack per metric, in the metric's unit.
SLACK = {
    "checkpoint_ms": 250.0,
    "fork_ms_p50": 150.0,
    "fork_ms_p95": 300.0,
    "wall_s": 1.0,
    "private_mb_per_fork": 32.0,
}


def compare(run: dict, base: dict, factor: float) -> tuple[list[str], list[str]]:
    """Returns (failures, report lines)."""
    fails: list[str] = []
    lines: list[str] = []

    def one(what: str, metric: str, got, want) -> None:
        if want is None:
            return
        if got is None:
            fails.append(f"{what} {metric}: not measured (baseline {want})")
            return
        limit = max(want * factor, want + SLACK.get(metric, 0.0))
        ok = got <= limit
        lines.append(
            f"{'ok  ' if ok else 'FAIL'} {what:<10} {metric:<20} {got:10.1f}  (baseline {want:.1f}, limit {limit:.1f})"
        )
        if not ok:
            fails.append(f"{what} {metric}: {got:.1f} > {limit:.1f} (baseline {want:.1f})")

    one("checkpoint", "checkpoint_ms", run.get("checkpoint_ms"), base.get("checkpoint_ms"))
    got_waves = {str(w.get("forks")): w for w in run.get("waves", [])}
    for n, bw in sorted(base.get("waves", {}).items(), key=lambda kv: int(kv[0])):
        rw = got_waves.get(n)
        if rw is None:
            fails.append(f"wave of {n} forks: not run")
            continue
        for metric, want in bw.items():
            one(f"{n} forks", metric, rw.get(metric), want)
    for n in sorted(set(got_waves) - set(base.get("waves", {})), key=int):
        lines.append(f"info {n} forks: no baseline ({json.dumps(got_waves[n])})")
    return fails, lines


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("run")
    ap.add_argument("baseline")
    ap.add_argument("--host", required=True, help="host class in the baseline")
    a = ap.parse_args(argv)
    with open(a.run) as f:
        run = json.load(f)
    with open(a.baseline) as f:
        baseline = json.load(f)
    base = baseline.get("hosts", {}).get(a.host)
    if base is None:
        print(f"no baseline for host {a.host!r} (have {sorted(baseline.get('hosts', {}))})")
        return 2
    if bool(run.get("forkable")) != bool(base.get("forkable", True)):
        print(f"run forkable={run.get('forkable')} but baseline is for forkable={base.get('forkable', True)}")
        return 2
    # A host class may set its own factor (shared CI runners are noisier).
    factor = float(base.get("factor", baseline.get("factor", 1.5)))
    fails, lines = compare(run, base, factor)
    print("\n".join(lines))
    if fails:
        print("\nregressions:\n  " + "\n  ".join(fails))
        return 1
    print("\nno regression")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
