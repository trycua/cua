#!/usr/bin/env python3
"""Evaluate IR-01: a permission-style sheet between step 1 and step 2 of a form (BenchLab irmodal).

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402
import interrupts as I  # noqa: E402

MODE = "irmodal"


def evaluate(seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks) -> None:
    exp = C.derive_irmodal(seed)
    answers_ev = [C.details(e).get("answer") for e in C.events_of(events, "permission_answer")]
    shown = bool(C.events_of(events, "permission_prompt_shown"))
    denied = shown and answers_ev == ["deny"] and state.get("prompt_answers") == ["deny"]
    checks.add(
        "permission_denied",
        denied,
        0.3,
        f"prompt shown {shown}; answers {answers_ev!r} (state {state.get('prompt_answers')!r})",
    )

    submits = C.events_of(events, "submit")
    last = C.details(submits[-1]).get("values", {}) if submits else {}
    got = state.get("submitted") if isinstance(state.get("submitted"), dict) else {}
    seats = C.parse_integer_result(I.norm(last.get("seats")))
    values_ok = (
        bool(submits)
        and I.norm(last.get("name")) == exp["name"]
        and I.norm(last.get("email")).lower() == exp["email"]
        and last.get("team") == exp["team"]
        and seats == exp["seats"]
        and got == last
    )
    checks.add(
        "fields_submitted",
        values_ok,
        0.4,
        f"last submit {last!r}; expected name {exp['name']!r}, email {exp['email']!r}, team {exp['team']!r}, seats {exp['seats']}",
    )
    checks.add("submitted_once", len(submits) == 1 and state.get("submit_count") == 1, 0.15, f"{len(submits)} submit(s)")
    checks.add(
        "state_matches_events",
        state.get("submit_count") == len(submits) and state.get("prompt_answers") == answers_ev,
        0.1,
        f"state submit_count {state.get('submit_count')!r} vs {len(submits)} events",
    )
    first_submit = I.seq_of(events, "submit")
    shown_seq = I.seq_of(events, "permission_prompt_shown")
    I.record(
        checks,
        kind="permission_prompt",
        shown=shown,
        exercised=shown and (first_submit is None or (shown_seq or 0) < first_submit),
        handled=denied if shown else None,
        completed=values_ok and len(submits) == 1,
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
