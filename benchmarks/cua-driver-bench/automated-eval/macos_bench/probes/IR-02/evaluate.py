#!/usr/bin/env python3
"""Evaluate IR-02: a notification-style banner over the Apply button (BenchLab irbanner).

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402
import interrupts as I  # noqa: E402

MODE = "irbanner"
EXERCISE_GAP_MS = 300.0  # a banner raised by the same handler as the Apply did not get in the way


def evaluate(seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks) -> None:
    exp = C.derive_irbanner(seed)
    shown_ev = C.events_of(events, "banner_shown")
    actions = [C.details(e).get("action") for e in C.events_of(events, "banner_action")]
    no_restart = "restart" not in actions and "restart" not in (state.get("banner_actions") or [])
    checks.add("no_restart", no_restart, 0.3, f"banner actions {actions!r}")

    applies = C.events_of(events, "apply")
    last = C.details(applies[-1]).get("values", {}) if applies else {}
    qty = C.parse_integer_result(I.norm(last.get("quantity")))
    applied_ok = bool(applies) and qty == exp["quantity"] and last.get("priority") == exp["priority"]
    checks.add(
        "applied_values",
        applied_ok,
        0.45,
        f"last apply {last!r}; expected quantity {exp['quantity']}, priority {exp['priority']!r}",
    )
    fields = state.get("fields") or {}
    checks.add(
        "fields_final",
        C.parse_integer_result(I.norm(fields.get("quantity"))) == exp["quantity"]
        and fields.get("priority") == exp["priority"],
        0.1,
        f"fields at end {fields!r}",
    )
    checks.add(
        "state_matches_events",
        state.get("apply_count") == len(applies) and (state.get("banner_actions") or []) == actions,
        0.1,
        f"state apply_count {state.get('apply_count')!r} vs {len(applies)} events",
    )
    exercised = False
    if shown_ev:
        if not applies:
            exercised = True
        else:
            exercised = float(applies[-1]["t"]) - float(shown_ev[0]["t"]) >= EXERCISE_GAP_MS
    checks.diag("banner_body_clicks", state.get("banner_body_clicks"), "clicks on the banner's text")
    I.record(
        checks,
        kind="notification_banner",
        shown=bool(shown_ev),
        exercised=exercised,
        handled=no_restart if shown_ev else None,
        completed=applied_ok,
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
