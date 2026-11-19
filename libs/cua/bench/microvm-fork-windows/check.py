#!/usr/bin/env python3
"""Perf guard for macOS microVM checkpoints and forks.

    python3 check.py results/summary.json baseline.json [--factor 1.5]

Fails (exit 1) when a measured p50 is more than `factor` times its baseline
(the baseline's own `factor` wins over the default), when a phase has
failed samples, or when memory per fork grows past its bound. Prints one
line per check.
"""

import argparse
import json
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("summary")
    ap.add_argument("baseline")
    ap.add_argument("--factor", type=float, default=None)
    a = ap.parse_args()
    summary = json.load(open(a.summary))
    base = json.load(open(a.baseline))
    factor = a.factor or base.get("factor", 1.5)
    failed = []
    for phase, want in base["p50_ms"].items():
        got = summary.get(phase)
        if not got or got.get("p50_ms") is None:
            failed.append(f"{phase}: no samples")
            continue
        if got["ok"] < got["n"]:
            failed.append(f"{phase}: {got['n'] - got['ok']} of {got['n']} samples failed")
        limit = want * factor
        ok = got["p50_ms"] <= limit
        print(f"{'ok  ' if ok else 'FAIL'} {phase}: p50 {got['p50_ms']:.0f} ms (baseline {want:.0f}, limit {limit:.0f})")
        if not ok:
            failed.append(f"{phase}: p50 {got['p50_ms']:.0f} ms > {limit:.0f} ms")
    mem = summary.get("memory")
    if "per_fork_mib" in base:
        if not mem:
            failed.append("memory: not measured")
        else:
            limit = base["per_fork_mib"] * factor
            ok = mem["per_fork_mib"] <= limit
            print(f"{'ok  ' if ok else 'FAIL'} memory: {mem['per_fork_mib']} MiB per fork (baseline {base['per_fork_mib']}, limit {limit:.0f})")
            if not ok:
                failed.append(f"memory: {mem['per_fork_mib']} MiB per fork > {limit:.0f}")
    for f in failed:
        print(f"regression: {f}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
