#!/usr/bin/env python3
"""Perf guard for the coordinator-to-Space channel.

Compares one `channel_bench` run (its `"summary": true` line) with a
committed baseline and fails when the channel regressed:

- a latency (`*_ms` metrics: p50 and p95) above `factor` x its baseline
  (default 1.5);
- a throughput (`MiBps_p50`, `MiBps`) below its baseline / `factor`;
- any absolute target of the baseline missed (`targets`, for example
  `exec_run_ms.p50 <= 2`);
- the run took another route than the baseline (`route_prefix`, for
  example `mesh direct`): a LAN run that fell back to the relay is a
  failure, not a slow pass.

Usage:
    check.py BASELINE.json SUMMARY.jsonl [--factor 1.5]
    check.py --self-test
    check.py --record SUMMARY.jsonl [--route-prefix P]   print a baseline

SUMMARY.jsonl is channel_bench's stdout; the last summary line is used.
Exit 0 when it holds, 1 with one line per regression otherwise.
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Dict, List, Optional


def summary_of(lines: List[str]) -> dict:
    found = None
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if row.get("summary"):
            found = row
    if found is None:
        raise SystemExit("no channel_bench summary line in the input")
    return found


def by_metric(summary: dict) -> Dict[str, dict]:
    return {r["metric"]: r for r in summary.get("results", []) if "metric" in r}


def value(rows: Dict[str, dict], path: str) -> Optional[float]:
    metric, _, field = path.rpartition(".")
    row = rows.get(metric)
    if row is None or row.get(field) is None:
        return None
    return float(row[field])


def check(baseline: dict, summary: dict, factor: float) -> List[str]:
    problems: List[str] = []
    want_route = baseline.get("route_prefix")
    route = summary.get("route") or ""
    if want_route and not route.startswith(want_route):
        problems.append(f"route {route!r} is not {want_route!r} (fell back to another path)")
    now = by_metric(summary)
    for metric, base in baseline.get("metrics", {}).items():
        row = now.get(metric)
        if row is None:
            problems.append(f"{metric}: missing from the run")
            continue
        for field in ("p50", "p95"):
            if field in base and metric.endswith("_ms"):
                limit = base[field] * factor
                got = row.get(field)
                if got is None or got > limit:
                    problems.append(
                        f"{metric}.{field} {got} ms > {limit:.3f} ms ({factor}x baseline {base[field]})"
                    )
        for field in ("MiBps_p50", "MiBps"):
            if field in base:
                floor = base[field] / factor
                got = row.get(field)
                if got is None or got < floor:
                    problems.append(
                        f"{metric}.{field} {got} MiB/s < {floor:.1f} MiB/s (baseline {base[field]} / {factor})"
                    )
    for path, bound in baseline.get("targets", {}).items():
        got = value(now, path)
        if got is None:
            problems.append(f"{path}: missing from the run (target {bound})")
            continue
        if "max" in bound and got > bound["max"]:
            problems.append(f"{path} {got} > target {bound['max']}")
        if "min" in bound and got < bound["min"]:
            problems.append(f"{path} {got} < target {bound['min']}")
    return problems


def record(summary: dict, route_prefix: Optional[str]) -> dict:
    """A baseline from one run: p50/p95 of every latency, MiB/s of every
    throughput, and the route's kind (`local`, `mesh direct`, ...)."""
    metrics: Dict[str, dict] = {}
    for metric, row in by_metric(summary).items():
        keep = {k: row[k] for k in ("p50", "p95", "MiBps_p50", "MiBps") if k in row}
        if metric.endswith("_ms"):
            keep = {k: v for k, v in keep.items() if k in ("p50", "p95", "MiBps_p50")}
        if keep:
            metrics[metric] = keep
    route = summary.get("route") or ""
    if route_prefix is None:
        words = route.split()
        route_prefix = " ".join(words[:2]) if words[:1] == ["mesh"] else (words[0] if words else "")
    return {
        "name": summary.get("label") or "baseline",
        "route_prefix": route_prefix,
        "metrics": metrics,
    }


def self_test() -> None:
    base = {
        "route_prefix": "local",
        "metrics": {
            "exec_run_ms": {"p50": 1.0, "p95": 2.0},
            "upload_100MiB_ms": {"MiBps_p50": 600},
        },
        "targets": {"exec_run_ms.p50": {"max": 2}, "upload_100MiB_ms.MiBps_p50": {"min": 500}},
    }
    good = {
        "summary": True,
        "route": "local 127.0.0.1:5",
        "results": [
            {"metric": "exec_run_ms", "p50": 1.2, "p95": 2.5},
            {"metric": "upload_100MiB_ms", "MiBps_p50": 550},
        ],
    }
    assert check(base, good, 1.5) == [], check(base, good, 1.5)
    slow = json.loads(json.dumps(good))
    slow["results"][0]["p50"] = 1.6
    assert any("exec_run_ms.p50" in p for p in check(base, slow, 1.5))
    thin = json.loads(json.dumps(good))
    thin["results"][1]["MiBps_p50"] = 300
    got = check(base, thin, 1.5)
    assert any("MiB/s <" in p for p in got) and any("target 500" in p for p in got), got
    relayed = json.loads(json.dumps(good))
    relayed["route"] = "relay https://relay.cua.ai"
    assert any("fell back" in p for p in check(base, relayed, 1.5))
    gone = json.loads(json.dumps(good))
    gone["results"] = gone["results"][:1]
    assert any("missing" in p for p in check(base, gone, 1.5))
    assert summary_of(["noise", json.dumps({"metric": "x"}), json.dumps(good)]) == good
    rec = record(good, None)
    assert rec["route_prefix"] == "local" and check(rec, good, 1.5) == [], rec
    meshy = dict(good, route="mesh direct 10.0.0.5:4242 rtt 0.2 ms")
    assert record(meshy, None)["route_prefix"] == "mesh direct"
    print("check.py self-test ok")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("baseline", nargs="?")
    ap.add_argument("summary", nargs="?")
    ap.add_argument("--factor", type=float, default=1.5)
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--record", metavar="SUMMARY")
    ap.add_argument("--route-prefix")
    a = ap.parse_args()
    if a.self_test:
        self_test()
        return 0
    if a.record:
        with open(a.record) as f:
            print(json.dumps(record(summary_of(f.readlines()), a.route_prefix), indent=2))
        return 0
    if not a.baseline or not a.summary:
        ap.error("BASELINE and SUMMARY are required")
    with open(a.baseline) as f:
        baseline = json.load(f)
    with open(a.summary) as f:
        summary = summary_of(f.readlines())
    problems = check(baseline, summary, a.factor)
    name = baseline.get("name", a.baseline)
    if problems:
        print(f"channel perf guard FAILED against {name}:")
        for p in problems:
            print(f"  - {p}")
        return 1
    print(f"channel perf guard ok against {name} (route {summary.get('route')!r})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
