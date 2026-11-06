#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Evaluate UM-01: the BenchLab desk task, done while the simulated user moves and clicks the mouse.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json [--sentinel SENTINEL.jsonl|.json]

Required (checker pass): integrity, instance_matches, code_correct, shelf_correct, submitted_once, submit_values,
no_foreign_input (benchlab_common.check_desk). The mouse user's measures (from the sentinel: largest displacement
from the script, clicks landed, lost or moved, foreign pointer events) are diagnostics here; the full pass reads
them through the Amendment 22 criteria (bench_v2.multiplayer_criteria). Without a mouse-user log that shows the
user ran (at least 5 scripted clicks and 50 checks) the result carries `harness_error`: re-run, never scored.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "desk"
MIN_CLICKS, MIN_CHECKS = 5, 50


def mouse_user_problem(sentinel: dict[str, Any] | None) -> str | None:
    """Why the mouse user cannot be scored from this sentinel summary, or None."""
    mu = (sentinel or {}).get("mouse_user")
    if not isinstance(mu, dict):
        return "no mouse-user log (the simulated user did not run)"
    if int(mu.get("clicks") or 0) < MIN_CLICKS or int(mu.get("checks") or 0) < MIN_CHECKS:
        return (
            f"the mouse user ran too little: {mu.get('clicks')} clicks, {mu.get('checks')} checks"
        )
    return None


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    C.check_desk(C.derive_um_desk(seed), [], state, events, checks)
    sentinel = checks.extra.get("sentinel")
    mu = (sentinel or {}).get("mouse_user")
    checks.diag("mouse_user", mu, "the simulated user's pointer and clicks (full pass reads these)")
    problem = mouse_user_problem(sentinel) if "sentinel" in checks.extra else None
    if problem:
        checks.diag("harness_error", problem, "the mouse user did not run: a harness error, re-run")


def main(argv: list[str] | None = None) -> int:
    code = C.run_cli(MODE, evaluate, argv)
    return code


if __name__ == "__main__":
    sys.exit(main())
