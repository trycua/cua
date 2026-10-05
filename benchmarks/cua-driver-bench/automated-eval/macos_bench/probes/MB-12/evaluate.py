#!/usr/bin/env python3
"""Evaluate MB-12: background operation. Fill three form fields in BenchLab while Bench Sentinel stays in front.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json --sentinel SUMMARY.json

This is the one task where disturbance is part of pass. SUMMARY.json is the output of
swift/summarize_sentinel.py for the armed window of the same trial.

Required task checks: customer_name, category, priority, submitted_once, ui_events (the submitted values
are explained by logged field edits).
Required disturbance checks: sentinel_available, front_unchanged (no change of frontmost app and the
sentinel kept key focus), pointer_still (largest pointer deviation <= 5 px), no_leaked_input (no key,
click or scroll reached the sentinel).
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "forms"
POINTER_LIMIT_PX = 5.0

EVENT_FIELDS = [
    ("customer_name", "customer_name"),
    ("invoice_amount", "invoice_amount"),
    ("category", "category_index"),
    ("priority", "priority"),
    ("notify", "notify"),
    ("quantity", "quantity"),
    ("notes", "notes"),
]
DEFAULTS = {
    "customer_name": C.FORMS_DEFAULTS["customer_name"],
    "invoice_amount": C.FORMS_DEFAULTS["invoice_amount"],
    "category": C.FORMS_DEFAULTS["category"],
    "priority": C.FORMS_DEFAULTS["priority"],
    "notify": C.FORMS_DEFAULTS["notify"],
    "quantity": C.FORMS_DEFAULTS["quantity"],
    "notes": C.FORMS_DEFAULTS["notes"],
}


def _ui_events(
    snap: dict[str, Any] | None, submits: list[dict[str, Any]], events: list[dict[str, Any]]
):
    if snap is None or not submits:
        return False, 0.1, "no submission snapshot or no submit event"
    last = submits[-1]
    if C.details(last).get("fields") != snap:
        return False, 0.1, "submit event fields differ from the state submission snapshot"
    replay = dict(DEFAULTS)
    for event in events:
        if event["seq"] >= last["seq"]:
            break
        d = C.details(event)
        if event.get("type") == "field_edit" and d.get("field") in replay:
            replay[d["field"]] = d.get("value")
    for event_field, snap_key in EVENT_FIELDS:
        if replay[event_field] != snap.get(snap_key):
            return (
                False,
                0.1,
                f"{event_field}: submitted {snap.get(snap_key)!r} but UI events leave {replay[event_field]!r}",
            )
    return True, 0.1, "submitted values match the logged field edits"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_forms(seed)
    sub = state.get("submission")
    snap = (
        sub.get("fields") if isinstance(sub, dict) and isinstance(sub.get("fields"), dict) else None
    )
    never = "form was never submitted"

    def field(name: str, ok: Any, detail: str) -> None:
        checks.add(name, snap is not None and bool(ok), 0.12, never if snap is None else detail)

    name = C.normalize_text(snap.get("customer_name")) if snap else None
    field(
        "customer_name",
        name == exp["customer_name"],
        f"got {name!r}, expected {exp['customer_name']!r}",
    )
    cat = snap.get("category_index") if snap else None
    field(
        "category",
        C.is_int(cat) and cat == exp["category_index"],
        f"got {cat!r}, expected {exp['category_index']} ({exp['category_title']})",
    )
    prio = snap.get("priority") if snap else None
    field("priority", prio == exp["priority"], f"got {prio!r}, expected {exp['priority']!r}")

    submits = C.events_of(events, "submit")
    once = (
        len(submits) == 1
        and state.get("submit_count") == 1
        and state.get("submitted") is True
        and state.get("status") == "Submitted"
    )
    checks.add(
        "submitted_once",
        once,
        0.1,
        f"{len(submits)} submit event(s), status {state.get('status')!r}",
    )
    checks.add("ui_events", *_ui_events(snap, submits, events))

    # Disturbance, part of pass for this task only.
    s = checks.extra.get("sentinel")
    available = isinstance(s, dict) and s.get("available") is True
    checks.add(
        "sentinel_available",
        available,
        0.1,
        "sentinel summary present and usable" if available else "no usable sentinel summary",
    )
    if not available:
        for n in ("front_unchanged", "pointer_still", "no_leaked_input"):
            checks.add(n, False, 0.1, "no sentinel data")
        return
    fc, kl = s.get("front_changes", 0), s.get("key_loss", 0)
    checks.add(
        "front_unchanged",
        fc == 0 and kl == 0,
        0.1,
        f"front_changes={fc}, key_loss={kl}, changed_to={s.get('front_changed_to')!r}",
    )
    dev = s.get("pointer_max_deviation_px", 0.0)
    checks.add(
        "pointer_still",
        C.is_num(dev) and dev <= POINTER_LIMIT_PX,
        0.1,
        f"largest pointer deviation {dev!r} px, limit {POINTER_LIMIT_PX}",
    )
    leaks = {k: s.get(k, 0) for k in ("keystrokes_leaked", "clicks_leaked", "scrolls_leaked")}
    checks.add(
        "no_leaked_input",
        all(v == 0 for v in leaks.values()),
        0.1,
        f"leaked to the front app: {leaks}",
    )
    checks.diag("hid_events", s.get("hid_events"))
    checks.diag("pointer_deviation_episodes", s.get("pointer_deviation_episodes"))


if __name__ == "__main__":
    C.main_guard(
        MODE,
        evaluate,
    )
