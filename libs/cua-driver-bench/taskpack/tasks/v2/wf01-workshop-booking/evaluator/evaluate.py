"""Evaluate WF-01: one new bandsaw booking with the stated details; the existing booking kept."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, web  # noqa: E402

TASK_ID = "WF-01"
EXISTING = "B-0417"
WANT = {"course": "bandsaw", "date": "2026-11-12", "slot": "14:00", "participants": 2, "member": "FH-30417"}
NOTE = "step-free access for one participant"


def evaluate(workspace: Path, checks: common.Checks) -> None:
    state, events = web.load(workspace)
    bookings = [b for b in state.get("bookings") or [] if isinstance(b, dict)]
    existing = [b for b in bookings if b.get("id") == EXISTING]
    new = [b for b in bookings if b.get("id") != EXISTING]
    submitted = web.of_type(events, "booking_submitted")
    one = new[0] if len(new) == 1 else {}
    checks.add("one_new_booking", len(new) == 1, f"{len(new)} new booking(s)")
    for key, want in WANT.items():
        checks.add(f"{key}_ok", one.get(key) == want, f"{key}={one.get(key)!r}, want {want!r}")
    note = web.norm(one.get("access_note")).lower().rstrip(".")
    checks.add("access_note_ok", one.get("access") is True and note == NOTE, f"access={one.get('access')!r} note={one.get('access_note')!r}")
    checks.add("booked_once", len(submitted) == 1, f"{len(submitted)} submission(s)")
    original = common.read_json(BUNDLE / "app" / "initial-state.json")["bookings"][0]
    checks.add("existing_booking_kept", existing == [original], f"existing {existing!r}", kind="preservation")
    checks.diag("submit_trusted", [e.get("trusted") for e in submitted])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
