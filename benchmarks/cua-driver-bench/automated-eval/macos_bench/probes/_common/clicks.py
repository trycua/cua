"""Click pairing for canvas modes: mouseDown/mouseUp events into clicks (copied from PROBE-CANVASCLICK)."""

from __future__ import annotations

import math
from typing import Any

import benchlab_common as C

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


def hit_circle(circles: list[dict[str, int]], x: float, y: float) -> tuple[int | None, bool]:
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
        index, edge = hit_circle(circles, x, y)
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


def same_click(state_click: Any, click: dict[str, Any]) -> bool:
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


def is_subsequence(needle: list[int], haystack: list[int]) -> bool:
    it = iter(haystack)
    return all(any(item == other for other in it) for item in needle)
