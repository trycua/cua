#!/usr/bin/env python3
"""Evaluate MB-02: right-click four circles on the BenchLab canvasmenu board, choose the stated action.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json

Required: circles_consistent, one check per target (the last action chosen on that circle equals the
stated one), no_wrong_actions (no action on a non-target circle), menu_via_right_press (every chosen
action follows a menu opened by a real right mouse press on the same circle), state_matches_events,
done_pressed, done_after_actions (plus integrity).

Diagnostics only, never part of pass: the doubled right-click event sequence (Down, Down, Up, Up;
trycua/cua#4679), ignored duplicate presses, dismissed menus, left clicks. The app ignores a second
rightMouseDown that arrives while a menu is pending, so doubling cannot decide the outcome.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402
import clicks as K  # noqa: E402

MODE = "canvasmenu"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_canvasmenu(seed)
    labels = {c["index"]: c["label"] for c in exp["circles"]}
    by_label = {c["label"]: c["index"] for c in exp["circles"]}
    expected = exp["expected"]  # label -> action

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

    actions = C.events_of(events, "menu_action")
    last: dict[int, str] = {}
    for e in actions:
        d = C.details(e)
        if C.is_int(d.get("circle")) and isinstance(d.get("action"), str):
            last[d["circle"]] = d["action"]

    for n, (label, action) in enumerate(expected.items(), start=1):
        got = last.get(by_label[label])
        checks.add(
            f"target_{n}_action",
            got == action,
            0.14,
            f"circle {label}: last action {got!r}, required {action!r}",
        )

    wrong = sorted(labels[i] for i in last if labels.get(i) not in expected)
    checks.add(
        "no_wrong_actions",
        not wrong,
        0.1,
        f"actions chosen on non-target circles {wrong}"
        if wrong
        else "no action on a non-target circle",
    )

    # Every action must follow a menu_open on the same circle, which follows a real right mouse press.
    mouse = C.events_of(events, "mouse")
    opens = C.events_of(events, "menu_open")
    genuine = bool(actions)
    for a in actions:
        circle = C.details(a).get("circle")
        o = [e for e in opens if C.details(e).get("circle") == circle and e["seq"] < a["seq"]]
        if not o:
            genuine = False
            break
        press = [
            m
            for m in mouse
            if C.details(m).get("phase") == "rightMouseDown"
            and C.details(m).get("circle") == circle
            and m["seq"] < o[-1]["seq"]
        ]
        if not press:
            genuine = False
            break
    checks.add(
        "menu_via_right_press",
        genuine,
        0.1,
        "each action follows a menu opened by a right mouse press on that circle"
        if genuine
        else "an action was not preceded by a right mouse press and menu on the same circle",
    )

    log = state.get("action_log")
    log_ok = isinstance(log, list) and [
        (x.get("circle"), x.get("action")) for x in log if isinstance(x, dict)
    ] == [(C.details(e).get("circle"), C.details(e).get("action")) for e in actions]
    per_circle_ok = circles_ok and all(
        (got.get("action") if got.get("index") in last else None) == last.get(got["index"])
        for got in sc
    )
    checks.add(
        "state_matches_events",
        log_ok and per_circle_ok,
        0.05,
        f"action_log ok={log_ok}, per-circle actions ok={per_circle_ok}",
    )

    done = C.events_of(events, "done_click")
    checks.add(
        "done_pressed",
        bool(done) and state.get("done") is True and state.get("done_count") == len(done),
        0.07,
        f"{len(done)} done_click event(s)",
    )
    target_actions = [e for e in actions if labels.get(C.details(e).get("circle")) in expected]
    last_seq = max((e["seq"] for e in target_actions), default=None)
    after = bool(done) and last_seq is not None and done[-1]["seq"] > last_seq
    checks.add(
        "done_after_actions",
        after,
        0.05,
        "Done was pressed after the last action"
        if after
        else "Done was not pressed after the last action",
    )

    clicks, anomalies = K.derive_clicks({"circles": exp["circles"]}, events)
    right_anoms = [x for x in anomalies if "right" in x.lower()]
    checks.diag("right_event_pairing_anomalies", len(right_anoms), "; ".join(right_anoms[:3]))
    checks.diag("right_down_duplicates_ignored", state.get("right_down_duplicates"))
    checks.diag("menu_opens", state.get("menu_opens"))
    checks.diag("menu_dismissed", len(C.events_of(events, "menu_dismiss")))
    checks.diag("left_clicks", sum(1 for c in clicks if c["button"] == "left"))
    checks.diag("action_events", len(actions))


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
