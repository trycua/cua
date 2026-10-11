#!/usr/bin/env python3
"""Evaluate MB-11: read a native tooltip, type its code, Submit once.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json

Tooltips are rect-based native tooltips (no per-icon accessibility element and no help attribute), and
the app logs `tooltip_query` when AppKit asks for the string, i.e. after a real hover that dwelt long
enough. Required: code_correct, submitted_once, tooltip_displayed_for_target (a tooltip_query for the
target icon precedes the Submit; a right code without it would mean the code leaked), value_via_events
(plus integrity). Diagnostics: which icons were queried, wrong-icon hovers.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "tooltip"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_tooltip(seed)
    submits = C.events_of(events, "submit")
    sv = C.details(submits[-1]).get("value") if submits else None
    got = sv.strip().upper() if isinstance(sv, str) else None
    checks.add(
        "code_correct",
        got == exp["target_code"],
        0.4,
        f"submitted {sv!r}, expected the code of the {exp['target']} icon",
    )
    once = (
        len(submits) == 1
        and state.get("submit_count") == 1
        and state.get("status") == "Submitted"
        and state.get("submitted_value") == sv
    )
    checks.add(
        "submitted_once",
        once,
        0.1,
        f"{len(submits)} submit event(s), status {state.get('status')!r}",
    )

    last = submits[-1]["seq"] if submits else None
    queries = [e for e in C.events_of(events, "tooltip_query") if last is None or e["seq"] < last]
    target_q = [e for e in queries if C.details(e).get("icon") == exp["target"]]
    checks.add(
        "tooltip_displayed_for_target",
        bool(target_q),
        0.3,
        f"{len(target_q)} tooltip display(s) of the {exp['target']} icon before Submit",
    )

    typed = None
    for e in events:
        if last is not None and e["seq"] >= last:
            break
        if e.get("type") == "field_edit" and C.details(e).get("field") == "code":
            typed = C.details(e).get("value")
    checks.add(
        "value_via_events",
        typed == sv and sv is not None,
        0.15,
        f"logged Code edits end at {typed!r}, submitted {sv!r}",
    )

    checks.diag("icons_queried", sorted({C.details(e).get("icon") for e in queries}))
    checks.diag(
        "wrong_icon_queries", sum(1 for e in queries if C.details(e).get("icon") != exp["target"])
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
