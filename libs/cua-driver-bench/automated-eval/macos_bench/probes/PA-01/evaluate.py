#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Evaluate PA-01: N agents, one per copy of BenchLab (`desk` mode), each doing the same short task.

Usage: evaluate.py --seed N --dir DIR --result OUT.json [--instances N] [--t0-ms T] [--budget-s S]

DIR holds one `desk-<label>/state.json` and `desk-<label>/events.jsonl` per copy and the runner's
`parallel.json` ({"instances", "t0_ms", "budget_s"}); the options override it. Per copy (prefixed `<label>.`):
integrity, instance_matches, code_correct, shelf_correct, submitted_once, submit_values, no_foreign_input and
in_budget (see benchlab_common.check_desk). The trial passes when every copy passes; `score` is the share of
copies that pass. Diagnostics: instances_passed, cross_talk_instances (copies with another copy's code letters in
them), foreign_shelf_selections, makespan_s (agents' start to the last passing Submit) and throughput_per_min.
A copy whose state or event file is missing means BenchLab did not start: `harness_error`, never a fail.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "desk"
BUDGET_S = 360


def desk_dir(root: Path, label: str) -> Path:
    return root / f"desk-{label}"


def evaluate_dir(
    seed: int, root: Path, instances: int, t0_ms: float | None, budget_s: float
) -> dict[str, Any]:
    plan = C.derive_parallel(seed, instances)
    cells = plan["cells"]
    deadline = None if t0_ms is None else t0_ms + budget_s * 1000.0
    total = C.Checks()
    per: dict[str, Any] = {}
    passed_submits: list[float] = []
    cross_talk: list[str] = []
    foreign_shelves = 0
    missing: list[str] = []
    for cell in cells:
        d = desk_dir(root, cell["label"])
        try:
            state = C.load_state(d / "state.json")
            events, bad = C.load_events(d / "events.jsonl")
        except C.LoadError as error:
            missing.append(f"{cell['label']}: {error}")
            continue
        others = [o for o in cells if o is not cell]
        found: dict[str, Any] = {}

        def check(s: int, st: dict[str, Any], ev: list[dict[str, Any]], ch: C.Checks) -> None:
            found.update(C.check_desk(cell, others, st, ev, ch, deadline_ms=deadline))

        result = C.evaluate_inputs(MODE, seed, state, events, bad, check)
        per[cell["label"]] = {
            "cell": cell["cell"],
            "passed": result["passed"],
            "score": result["score"],
            "checks": result["checks"],
        }
        for name, item in result["checks"].items():
            total.add(f"{cell['label']}.{name}", item["passed"], item["weight"], item["detail"])
        if found.get("foreign_input"):
            cross_talk.append(cell["label"])
        foreign_shelves += len(found.get("foreign_shelf_selections") or [])
        if result["passed"]:
            submits = C.events_of(events, "submit")
            if submits and C.is_num(submits[-1].get("t")):
                passed_submits.append(float(submits[-1]["t"]))
    if missing:
        out = C.failed_result("instances_started", "; ".join(missing))
        out["harness_error"] = "BenchLab copy did not start"
        out["error"] = "; ".join(missing)
        return out
    out = total.result()
    k = sum(1 for v in per.values() if v["passed"])
    out["passed"] = k == instances and all(v["passed"] for v in per.values())
    out["score"] = round(k / instances, 4)
    makespan = (
        round((max(passed_submits) - t0_ms) / 1000.0, 2)
        if passed_submits and t0_ms is not None
        else None
    )
    throughput = (
        round(k / (makespan / 60.0), 3) if makespan and makespan > 0 else (0.0 if k == 0 else None)
    )
    out["instances"] = per
    out["diagnostics"] = {
        "instances": {"value": instances, "detail": "copies of BenchLab, one agent each"},
        "instances_passed": {"value": k, "detail": f"{k}/{instances} copies passed every check"},
        "cross_talk_instances": {
            "value": len(cross_talk),
            "detail": f"copies holding another copy's code letters: {cross_talk}",
        },
        "foreign_shelf_selections": {
            "value": foreign_shelves,
            "detail": "selections of another copy's target shelf (not attributable; not part of pass)",
        },
        "makespan_s": {"value": makespan, "detail": "agents' start to the last passing Submit"},
        "throughput_per_min": {
            "value": throughput,
            "detail": "passing copies per minute of makespan",
        },
        "parallel": {
            "value": {
                "instances": instances,
                "passed": k,
                "cross_talk": len(cross_talk),
                "cross_talk_labels": cross_talk,
                "makespan_s": makespan,
                "throughput_per_min": throughput,
                "budget_s": budget_s,
            }
        },
    }
    return out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Evaluate a PA-01 trial.")
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--result", required=True)
    parser.add_argument("--instances", type=int)
    parser.add_argument("--t0-ms", type=float)
    parser.add_argument("--budget-s", type=float)
    args = parser.parse_args(argv)
    meta: dict[str, Any] = {}
    try:
        meta = json.loads((args.dir / "parallel.json").read_text("utf-8"))
    except (OSError, json.JSONDecodeError):
        pass
    instances = int(args.instances or meta.get("instances") or C.PA_DEFAULT_INSTANCES)
    t0 = args.t0_ms if args.t0_ms is not None else meta.get("t0_ms")
    budget = float(args.budget_s or meta.get("budget_s") or BUDGET_S)
    try:
        result = evaluate_dir(args.seed, args.dir, instances, t0, budget)
    except Exception as error:  # noqa: BLE001 - always write a result
        result = C.failed_result("evaluator_error", f"{type(error).__name__}: {error}")
    C.write_result(args.result, result)
    print(json.dumps({"passed": result["passed"], "score": result["score"]}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
