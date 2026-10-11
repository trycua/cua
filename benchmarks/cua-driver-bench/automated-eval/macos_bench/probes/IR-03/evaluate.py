#!/usr/bin/env python3
"""Evaluate IR-03: an unsaved-changes sheet when switching notes or pressing Done (BenchLab irunsaved).

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402
import interrupts as I  # noqa: E402

MODE = "irunsaved"


def evaluate(seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks) -> None:
    exp = C.derive_irunsaved(seed)
    answers = [C.details(e).get("answer") for e in C.events_of(events, "save_answer")]
    discards = C.events_of(events, "discard")
    kept = "dont_save" not in answers and not discards and "dont_save" not in (state.get("answers") or [])
    checks.add("no_work_discarded", kept, 0.3, f"save answers {answers!r}")

    saved = state.get("saved") if isinstance(state.get("saved"), list) else []
    ok_shape = len(saved) == 3 and all(isinstance(n, dict) for n in saved)
    si, ri = exp["status_index"], exp["rename_index"]
    status_ok = ok_shape and I.norm(saved[si].get("status")) == exp["status"]
    title_ok = ok_shape and I.norm(saved[ri].get("title")) == exp["new_title"]
    checks.add("status_saved", status_ok, 0.2, f"saved note {si}: {saved[si] if ok_shape else None!r}")
    checks.add("title_saved", title_ok, 0.2, f"saved note {ri}: {saved[ri] if ok_shape else None!r}")
    others = True
    if ok_shape:
        for i, note in enumerate(saved):
            want_title = exp["new_title"] if i == ri else exp["titles"][i]
            want_status = exp["status"] if i == si else C.IR_INITIAL_STATUS
            if I.norm(note.get("title")) != want_title or I.norm(note.get("status")) != want_status:
                others = False
            if note.get("body") != f"Working notes for {exp['titles'][i]}.":
                others = False
    checks.add("others_unchanged", ok_shape and others, 0.1, "every other title, status and body as at start")
    done = C.events_of(events, "done")
    done_ok = bool(done) and C.details(done[-1]).get("clean") is True and state.get("dirty") is False
    checks.add("done_clean", done_ok, 0.1, f"{len(done)} done event(s); dirty at end {state.get('dirty')!r}")
    checks.add(
        "state_matches_events",
        (state.get("answers") or []) == answers and state.get("done_count") == len(done),
        0.05,
        f"state answers {state.get('answers')!r}",
    )
    prompts = C.events_of(events, "save_prompt_shown")
    last_done = done[-1]["seq"] if done else None
    I.record(
        checks,
        kind="unsaved_changes_dialog",
        shown=bool(prompts),
        exercised=bool(prompts) and (last_done is None or prompts[0]["seq"] < last_done),
        handled=kept if prompts else None,
        completed=status_ok and title_ok and others and done_ok,
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
