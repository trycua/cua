#!/usr/bin/env python3
"""Evaluate MB-01: 12 left clicks in order on the BenchLab canvasclick board, then Done.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json

Required: circles_consistent, state_matches_events, left_sequence_in_order, no_extra_left_clicks,
done_pressed, done_after_clicks (plus integrity). Diagnostics: event pairing anomalies, misses, any
right clicks. Clicks are derived from real mouseDown/mouseUp events logged by the app; the board has
no per-circle accessibility children, so no other route registers a click.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402
import clicks as K  # noqa: E402

MODE = "canvasclick"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_canvasclick(seed)
    required = exp["left_sequence"]
    found, anomalies = K.derive_clicks(exp, events)

    sc = state.get("circles")
    circles_ok = isinstance(sc, list) and len(sc) == len(exp["circles"])
    if circles_ok:
        for want, got in zip(exp["circles"], sc):
            circles_ok = (
                circles_ok
                and isinstance(got, dict)
                and all(got.get(k) == want[k] for k in ("index", "label", "cx", "cy"))
            )
    checks.add(
        "circles_consistent",
        circles_ok,
        0.05,
        "state circles match the seed layout"
        if circles_ok
        else "state circles differ from the seed layout",
    )

    log = state.get("click_log")
    log_ok = (
        isinstance(log, list)
        and len(log) == len(found)
        and all(K.same_click(s, c) for s, c in zip(log, found))
    )
    counts_ok = circles_ok and all(
        got.get("left_clicks")
        == sum(1 for c in found if c["button"] == "left" and c["circle"] == got["index"])
        and got.get("right_clicks")
        == sum(1 for c in found if c["button"] == "right" and c["circle"] == got["index"])
        for got in sc
    )
    checks.add(
        "state_matches_events",
        log_ok and counts_ok,
        0.05,
        f"click_log ok={log_ok}, counts ok={counts_ok} ({len(found)} click(s) in events)",
    )

    left = [c["label"] for c in found if c["button"] == "left" and c["circle"] is not None]
    misses = sum(1 for c in found if c["button"] == "left" and c["circle"] is None)
    checks.add(
        "left_sequence_in_order",
        K.is_subsequence(required, left),
        0.3,
        f"left clicks on circles {left}, required {required}",
    )
    checks.add(
        "no_extra_left_clicks",
        left == required,
        0.2,
        f"{len(left)} left click(s) on circles for {len(required)} required; {misses} missed every circle (allowed)",
    )

    done = C.events_of(events, "done_click")
    checks.add(
        "done_pressed",
        bool(done) and state.get("done") is True and state.get("done_count") == len(done),
        0.1,
        f"{len(done)} done_click event(s)",
    )
    needed = [
        c
        for c in found
        if c["button"] == "left" and c["circle"] is not None and c["label"] in required
    ]
    last_needed = max((c["seq"] for c in needed), default=None)
    after = bool(done) and last_needed is not None and done[-1]["seq"] > last_needed
    checks.add(
        "done_after_clicks",
        after,
        0.1,
        "Done was pressed after the last required click"
        if after
        else "Done was not pressed after the last required click",
    )

    checks.diag("event_pairing_anomalies", len(anomalies), "; ".join(anomalies[:3]))
    checks.diag("left_misses", misses)
    checks.diag("right_clicks", sum(1 for c in found if c["button"] == "right"))


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
