#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Evaluate PA-02: the crowded desk (up to 16 overlapping copies of BenchLab `desk` mode, one agent each).

Usage: evaluate.py --seed N --dir DIR --result OUT.json [--instances N] [--t0-ms T] [--budget-s S]
                   [--copy-deadline-s S]

Same layout and result as PA-01 (probes/PA-01/evaluate.py), with the hard variant's rules: the copies come from
`derive_crowded`, each copy must also pass `confirm_before_submit` and `confirm_resolved`, cross-talk is a tag or
a 5-character fragment of another copy's code, and every copy's own deadline (`copy_deadline_s`, default 180 s
from the agents' start) replaces the trial budget in `in_budget`. The trial budget (240 s) still ends the agents.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

_spec = importlib.util.spec_from_file_location(
    "pa01_evaluate", Path(__file__).resolve().parent.parent / "PA-01" / "evaluate.py"
)
assert _spec and _spec.loader
PA01 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(PA01)

BUDGET_S = C.PA2_BUDGET_S
COPY_DEADLINE_S = C.PA2_COPY_DEADLINE_S


def evaluate_dir(
    seed: int,
    root: Path,
    instances: int,
    t0_ms: float | None,
    budget_s: float = BUDGET_S,
    copy_deadline_s: float = COPY_DEADLINE_S,
) -> dict[str, Any]:
    return PA01.evaluate_dir(
        seed, root, instances, t0_ms, budget_s, hard=True, copy_deadline_s=copy_deadline_s
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Evaluate a PA-02 trial.")
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--result", required=True)
    parser.add_argument("--instances", type=int)
    parser.add_argument("--t0-ms", type=float)
    parser.add_argument("--budget-s", type=float)
    parser.add_argument("--copy-deadline-s", type=float)
    args = parser.parse_args(argv)
    meta: dict[str, Any] = {}
    try:
        meta = json.loads((args.dir / "parallel.json").read_text("utf-8"))
    except (OSError, json.JSONDecodeError):
        pass
    instances = int(args.instances or meta.get("instances") or C.PA2_DEFAULT_INSTANCES)
    t0 = args.t0_ms if args.t0_ms is not None else meta.get("t0_ms")
    budget = float(args.budget_s or meta.get("budget_s") or BUDGET_S)
    deadline = float(args.copy_deadline_s or meta.get("copy_deadline_s") or COPY_DEADLINE_S)
    try:
        result = evaluate_dir(args.seed, args.dir, instances, t0, budget, deadline)
    except Exception as error:  # noqa: BLE001 - always write a result
        result = C.failed_result("evaluator_error", f"{type(error).__name__}: {error}")
    C.write_result(args.result, result)
    print(json.dumps({"passed": result["passed"], "score": result["score"]}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
