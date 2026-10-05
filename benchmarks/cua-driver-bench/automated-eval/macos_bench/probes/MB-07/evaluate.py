#!/usr/bin/env python3
"""Evaluate MB-07 (was PROBE-CLIPBOARD): BenchLab clipboard mode.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "clipboard"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_clipboard(seed)
    saves = C.events_of(events, "save")
    saved_value = state.get("saved_value")

    saved_ok = (
        state.get("saved") is True
        and isinstance(saved_value, str)
        and saved_value.strip() != ""
        and bool(saves)
        and state.get("save_count") == len(saves)
    )
    checks.add(
        "result_saved", saved_ok, 0.2, f"{len(saves)} save event(s), saved_value={saved_value!r}"
    )

    parsed = C.parse_integer_result(saved_value)
    checks.add(
        "result_correct",
        parsed is not None and parsed == exp["result"],
        0.5,
        f"saved {saved_value!r} parsed as {parsed!r}, expected {exp['a']} x {exp['b']} + {exp['c']} = {exp['result']}",
    )

    checks.add("ui_events", *_ui_events(state, events, saves, saved_value))

    # Diagnostics, never part of pass. The prompt asks for Calculator, copy and paste; the final value
    # is scored, not the route. The runner records whether Calculator was running at the end.
    edits = [
        C.details(e)
        for e in C.events_of(events, "field_edit")
        if C.details(e).get("field") == "result"
    ]
    checks.diag(
        "pasteboard_changes",
        state.get("pasteboard_changes"),
        "NSPasteboard change count since app start",
    )
    checks.diag(
        "whole_value_in_one_edit",
        bool(edits)
        and any(len(str(d.get("value", ""))) >= 4 and d.get("via") != "typing" for d in edits[:2]),
        "value arrived in one edit, as with a paste or a value set",
    )
    checks.diag("save_count", len(saves))


def _ui_events(
    state: dict[str, Any],
    events: list[dict[str, Any]],
    saves: list[dict[str, Any]],
    saved_value: Any,
):
    weight = 0.25
    if not saves or not isinstance(saved_value, str):
        return False, weight, "no save event"
    last = saves[-1]
    if C.details(last).get("value") != saved_value:
        return False, weight, "save event value differs from state saved_value"
    typed = None
    for event in events:
        if event["seq"] >= last["seq"]:
            break
        d = C.details(event)
        if event.get("type") == "field_edit" and d.get("field") == "result":
            typed = d.get("value")
    if typed != saved_value:
        return False, weight, f"saved {saved_value!r} but the logged Result edits end at {typed!r}"
    return (
        True,
        weight,
        f"saved value matches the logged Result edits (pasteboard changes during run: {state.get('pasteboard_changes')!r})",
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
