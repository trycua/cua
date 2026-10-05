#!/usr/bin/env python3
"""Evaluate MB-03: select the one row with the stated Name and Qty in a 400-row table, press Confirm.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json

Required: confirm_pressed, selected_target_at_confirm, no_wrong_confirm, selection_events_consistent,
state_matches (plus integrity). Diagnostics: whether the viewport scrolled near the target row, scroll
and selection event counts. The target row is always below row 200, so it cannot be selected without
scrolling; selection through any route that raises a selection change counts.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "tablesel"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_tablesel(seed)
    target = exp["code"]
    confirms = C.events_of(events, "confirm")
    last = confirms[-1] if confirms else None
    last_sel = C.details(last).get("selected_code") if last else None

    pressed = (
        bool(confirms)
        and state.get("confirmed") is True
        and state.get("confirm_count") == len(confirms)
        and state.get("status") == "Confirmed"
    )
    checks.add(
        "confirm_pressed",
        pressed,
        0.15,
        f"{len(confirms)} confirm event(s), status {state.get('status')!r}",
    )
    checks.add(
        "selected_target_at_confirm",
        last_sel == target,
        0.4,
        f"selected {last_sel!r} at the last Confirm, target {target!r} (Name {exp['name']!r}, Qty {exp['qty']})",
    )
    wrong = [
        C.details(e).get("selected_code")
        for e in confirms
        if C.details(e).get("selected_code") != target
    ]
    checks.add(
        "no_wrong_confirm",
        bool(confirms) and not wrong,
        0.15,
        f"Confirm pressed with {wrong} selected"
        if wrong
        else f"{len(confirms)} Confirm press(es), all on the target",
    )

    selects = [
        e for e in C.events_of(events, "row_select") if last is None or e["seq"] < last["seq"]
    ]
    sel_ok = bool(selects) and C.details(selects[-1]).get("code") == target and last_sel == target
    checks.add(
        "selection_events_consistent",
        sel_ok,
        0.15,
        "the last selection event before Confirm is the target"
        if sel_ok
        else "no selection event of the target row precedes the last Confirm",
    )
    checks.add(
        "state_matches",
        state.get("selected_code") == target and last_sel == target,
        0.1,
        f"state selected_code {state.get('selected_code')!r}",
    )

    row0 = exp["row"] - 1
    reach = state.get("max_first_visible_row")
    checks.diag(
        "scrolled_near_target",
        C.is_int(reach) and reach >= row0 - 25,
        f"max first visible row {reach!r}, target row index {row0}",
    )
    checks.diag("scroll_events", len(C.events_of(events, "scroll")))
    checks.diag("select_events", len(C.events_of(events, "row_select")))
    checks.diag("confirm_count", len(confirms))


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
