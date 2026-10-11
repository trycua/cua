#!/usr/bin/env python3
"""Evaluate PROBE-CANVASCLICK: BenchLab canvasclick mode.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import math
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "canvasclick"
COORD_TOLERANCE = 1e-6
BOUNDARY_SLACK = 1e-6  # hit tests exactly on the circle edge may round differently

PHASES = {
    "mouseDown": ("left", True),
    "mouseUp": ("left", False),
    "rightMouseDown": ("right", True),
    "rightMouseUp": ("right", False),
    "otherMouseDown": ("other", True),
    "otherMouseUp": ("other", False),
}


def _hit(circles: list[dict[str, int]], x: float, y: float) -> tuple[int | None, bool]:
    """Circle index under (x, y) and whether the point sits within slack of an edge."""
    found, edge = None, False
    for c in circles:
        dist = math.hypot(x - c["cx"], y - c["cy"])
        if dist <= C.CIRCLE_RADIUS:
            found = c["index"]
        if abs(dist - C.CIRCLE_RADIUS) < BOUNDARY_SLACK:
            edge = True
    return found, edge


def derive_clicks(
    exp: dict[str, Any], events: list[dict[str, Any]]
) -> tuple[list[dict[str, Any]], list[str]]:
    """Pair mouseDown/mouseUp events into clicks; return (clicks, anomalies)."""
    circles = exp["circles"]
    labels = {c["index"]: c["label"] for c in circles}
    clicks: list[dict[str, Any]] = []
    anomalies: list[str] = []
    pending: dict[str, dict[str, Any]] = {}
    for event in C.events_of(events, "mouse"):
        d = C.details(event)
        seq = event.get("seq")
        phase = d.get("phase")
        if phase not in PHASES or not C.is_num(d.get("x")) or not C.is_num(d.get("y")):
            anomalies.append(f"seq {seq}: unusable mouse event {phase!r}")
            continue
        button, is_down = PHASES[phase]
        if d.get("button") != button:
            anomalies.append(f"seq {seq}: phase {phase} with button {d.get('button')!r}")
            continue
        x, y = float(d["x"]), float(d["y"])
        index, edge = _hit(circles, x, y)
        logged = d.get("circle")
        if logged != index and not edge:
            anomalies.append(f"seq {seq}: logged circle {logged!r} but the point is over {index!r}")
            continue
        if logged is not None and d.get("label") != labels.get(logged):
            anomalies.append(f"seq {seq}: logged label {d.get('label')!r} for circle {logged!r}")
            continue
        circle = logged
        if is_down:
            if button in pending:
                anomalies.append(f"seq {pending[button]['seq']}: {button} mouseDown never released")
            pending[button] = {"x": x, "y": y, "circle": circle, "seq": seq}
        else:
            down = pending.pop(button, None)
            if down is None:
                anomalies.append(f"seq {seq}: {phase} without a matching mouseDown")
                continue
            hit = (
                down["circle"] if down["circle"] is not None and down["circle"] == circle else None
            )
            clicks.append(
                {
                    "button": button,
                    "circle": hit,
                    "label": labels[hit] if hit is not None else None,
                    "x": down["x"],
                    "y": down["y"],
                    "seq": seq,
                }
            )
    for button, down in pending.items():
        anomalies.append(f"seq {down['seq']}: {button} mouseDown never released")
    return clicks, anomalies


def _same_click(state_click: Any, click: dict[str, Any]) -> bool:
    return (
        isinstance(state_click, dict)
        and state_click.get("button") == click["button"]
        and state_click.get("circle") == click["circle"]
        and state_click.get("label") == click["label"]
        and C.is_num(state_click.get("x"))
        and C.is_num(state_click.get("y"))
        and abs(state_click["x"] - click["x"]) < COORD_TOLERANCE
        and abs(state_click["y"] - click["y"]) < COORD_TOLERANCE
    )


def _is_subsequence(needle: list[int], haystack: list[int]) -> bool:
    it = iter(haystack)
    return all(any(item == other for other in it) for item in needle)


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_canvasclick(seed)
    required_left = exp["left_sequence"]
    right_targets = exp["right_targets"]
    clicks, anomalies = derive_clicks(exp, events)

    state_circles = state.get("circles")
    circles_ok = isinstance(state_circles, list) and len(state_circles) == len(exp["circles"])
    if circles_ok:
        for want, got in zip(exp["circles"], state_circles):
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

    checks.add(
        "mouse_pairs",
        not anomalies and bool(clicks),
        0.1,
        "; ".join(anomalies[:3])
        if anomalies
        else f"{len(clicks)} click(s), each a paired mouseDown and mouseUp",
    )

    state_log = state.get("click_log")
    log_ok = (
        isinstance(state_log, list)
        and len(state_log) == len(clicks)
        and all(_same_click(s, c) for s, c in zip(state_log, clicks))
    )
    counts_ok = circles_ok and all(
        got.get("left_clicks")
        == sum(1 for c in clicks if c["button"] == "left" and c["circle"] == got["index"])
        and got.get("right_clicks")
        == sum(1 for c in clicks if c["button"] == "right" and c["circle"] == got["index"])
        for got in state_circles
    )
    checks.add(
        "state_matches_events",
        log_ok and counts_ok,
        0.05,
        "state click log and per-circle counts equal the clicks in the events"
        if log_ok and counts_ok
        else f"click_log ok={log_ok}, counts ok={counts_ok} ({len(clicks)} click(s) in events)",
    )

    left_labels = [c["label"] for c in clicks if c["button"] == "left" and c["circle"] is not None]
    left_misses = sum(1 for c in clicks if c["button"] == "left" and c["circle"] is None)
    checks.add(
        "left_sequence_in_order",
        _is_subsequence(required_left, left_labels),
        0.25,
        f"left clicks on circles {left_labels}, required {required_left}",
    )
    checks.add(
        "no_extra_left_clicks",
        left_labels == required_left,
        0.15,
        f"{len(left_labels)} left click(s) on circles for {len(required_left)} required; "
        f"{left_misses} left click(s) missed every circle (allowed)",
    )

    right_labels = [
        c["label"] for c in clicks if c["button"] == "right" and c["circle"] is not None
    ]
    missing = [t for t in right_targets if t not in right_labels]
    checks.add(
        "right_targets",
        not missing,
        0.15,
        f"right-clicked {right_labels}, targets {right_targets}"
        + (f", missing {missing}" if missing else ""),
    )
    wrong = [label for label in right_labels if label not in right_targets]
    right_misses = sum(1 for c in clicks if c["button"] == "right" and c["circle"] is None)
    checks.add(
        "no_wrong_right_clicks",
        not wrong,
        0.05,
        f"right clicks on non-target circles {wrong}; {right_misses} right click(s) missed every circle (allowed)",
    )

    done_events = C.events_of(events, "done_click")
    checks.add(
        "done_pressed",
        bool(done_events)
        and state.get("done") is True
        and state.get("done_count") == len(done_events),
        0.1,
        f"{len(done_events)} done_click event(s), state done={state.get('done')!r}",
    )
    needed = [
        c
        for c in clicks
        if c["circle"] is not None
        and (
            (c["button"] == "left" and c["label"] in required_left)
            or (c["button"] == "right" and c["label"] in right_targets)
        )
    ]
    last_needed = max((c["seq"] for c in needed), default=None)
    after = bool(done_events) and last_needed is not None and done_events[-1]["seq"] > last_needed
    checks.add(
        "done_after_clicks",
        after,
        0.05,
        "Done was pressed after the last required click"
        if after
        else "Done was not pressed after the last required click",
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
