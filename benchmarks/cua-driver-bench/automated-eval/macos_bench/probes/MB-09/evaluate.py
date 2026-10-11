#!/usr/bin/env python3
"""Evaluate MB-09: reorder a native list by drag and drop, then Done.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json

Required: order_correct (final order equals the stated order), done_pressed, done_order_correct (the
order at the last Done press), moves_via_drop (replaying the logged drop events from the seed's initial
order reproduces every logged order and the final state; drops are only raised by a real drag session),
initial_consistent (plus integrity). Diagnostics: drag sessions begun, number of drops.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "listdrag"


def _replay(initial: list[str], drops: list[dict[str, Any]]) -> tuple[list[str], bool]:
    order = list(initial)
    for e in drops:
        d = C.details(e)
        f, t, item = d.get("from"), d.get("to"), d.get("item")
        if not (C.is_int(f) and C.is_int(t) and 0 <= f < len(order)) or order[f] != item:
            return order, False
        order.pop(f)
        order.insert(min(max(t, 0), len(order)), item)
        if d.get("order") != order:
            return order, False
    return order, True


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_listdrag(seed)
    order = state.get("order")
    checks.add(
        "order_correct",
        order == exp["target"],
        0.35,
        f"final order {order!r}, expected {exp['target']!r}",
    )

    done = C.events_of(events, "done_click")
    checks.add(
        "done_pressed",
        bool(done) and state.get("done") is True and state.get("done_count") == len(done),
        0.1,
        f"{len(done)} done_click event(s)",
    )
    done_order = C.details(done[-1]).get("order") if done else None
    checks.add(
        "done_order_correct",
        done_order == exp["target"] and state.get("done_order") == done_order,
        0.15,
        f"order at the last Done {done_order!r}",
    )

    drops = C.events_of(events, "reorder")
    replayed, ok = _replay(exp["initial"], drops)
    via = bool(drops) and ok and replayed == order and state.get("move_count") == len(drops)
    checks.add(
        "moves_via_drop",
        via,
        0.3,
        f"{len(drops)} drop event(s); replay ok={ok}, replay result matches state={replayed == order}",
    )

    first_from = C.details(drops[0]).get("from") if drops else None
    init_ok = (
        bool(drops)
        and C.is_int(first_from)
        and 0 <= first_from < len(exp["initial"])
        and C.details(drops[0]).get("item") == exp["initial"][first_from]
    )
    checks.add(
        "initial_consistent",
        init_ok,
        0.05,
        "the first drop moves an item of the seed's initial order"
        if init_ok
        else "the first drop does not start from the seed's initial order",
    )

    checks.diag("drag_sessions_begun", state.get("drag_begins"))
    checks.diag("drops", len(drops))


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
