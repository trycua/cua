#!/usr/bin/env python3
"""Evaluate PROBE-HOVER: BenchLab hover mode.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "hover"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_hover(seed)
    target = exp["target"]
    clicks = C.events_of(events, "overlay_click")
    last = clicks[-1] if clicks else None
    last_d = C.details(last) if last else {}
    state_clicks = state.get("clicks") if isinstance(state.get("clicks"), list) else []

    checks.add(
        "overlay_names_consistent",
        state.get("overlay_names") == exp["names"],
        0.05,
        f"state overlay_names {state.get('overlay_names')!r}, seed layout {exp['names']!r}",
    )

    clicked_ok = (
        last is not None
        and last_d.get("name") == target
        and state.get("last_clicked") == target
        and [c.get("name") for c in state_clicks if isinstance(c, dict)]
        == [C.details(e).get("name") for e in clicks]
    )
    checks.add(
        "clicked_target",
        clicked_ok,
        0.25,
        f"last click {last_d.get('name')!r} (state {state.get('last_clicked')!r}), target {target!r}",
    )

    visible_ok = (
        last is not None
        and last_d.get("overlay_visible") is True
        and state.get("last_click_overlay_visible") is True
    )
    checks.add(
        "overlay_visible_at_click",
        visible_ok,
        0.2,
        f"overlay_visible at last click: event {last_d.get('overlay_visible')!r}, state {state.get('last_click_overlay_visible')!r}",
    )

    checks.add("hover_genuine", *_hover_genuine(state, events, last))

    wrong = [C.details(e).get("name") for e in clicks if C.details(e).get("name") != target]
    checks.add(
        "no_wrong_clicks",
        bool(clicks) and not wrong,
        0.15,
        f"wrong clicks: {wrong}" if wrong else f"{len(clicks)} click(s), none wrong",
    )
    checks.add("single_click", len(clicks) == 1, 0.1, f"{len(clicks)} overlay click(s)")


def _hover_genuine(
    state: dict[str, Any], events: list[dict[str, Any]], last: dict[str, Any] | None
):
    weight = 0.2
    if last is None:
        return False, weight, "no overlay click"
    mouse = [e for e in C.events_of(events, "mouse") if C.details(e).get("region") == "hot_zone"]
    entered = [e for e in mouse if C.details(e).get("phase") == "mouseEntered"]
    exited = [e for e in mouse if C.details(e).get("phase") == "mouseExited"]
    hover_before = [
        e
        for e in mouse
        if e["seq"] < last["seq"] and C.details(e).get("phase") in ("mouseEntered", "mouseMoved")
    ]
    shown_before = [e for e in C.events_of(events, "overlay_shown") if e["seq"] < last["seq"]]
    counts_ok = state.get("hover_enter_count") == len(entered) and state.get(
        "hover_exit_count"
    ) == len(exited)
    ok = bool(hover_before) and bool(shown_before) and counts_ok
    return (
        ok,
        weight,
        f"{len(hover_before)} hot-zone hover event(s) and {len(shown_before)} overlay_shown event(s) before the click; "
        f"enter/exit counts {state.get('hover_enter_count')!r}/{state.get('hover_exit_count')!r} vs events {len(entered)}/{len(exited)}",
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
