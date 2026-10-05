#!/usr/bin/env python3
"""Evaluate PROBE-FORMS: BenchLab forms mode.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import sys
from decimal import Decimal
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "forms"

# (event field name, snapshot key)
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


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_forms(seed)
    submission = state.get("submission")
    snap = submission.get("fields") if isinstance(submission, dict) else None
    if not isinstance(snap, dict):
        snap = None
    never = "form was never submitted"

    def field(name: str, ok: Any, detail: str) -> None:
        checks.add(name, snap is not None and bool(ok), 0.1, never if snap is None else detail)

    name = C.normalize_text(snap.get("customer_name")) if snap else None
    field(
        "customer_name",
        name == exp["customer_name"],
        f"got {name!r}, expected {exp['customer_name']!r}",
    )

    amount = C.parse_amount(snap.get("invoice_amount")) if snap else None
    amount_ok = amount is not None and abs(amount - Decimal(exp["invoice_amount"])) < Decimal(
        "0.005"
    )
    field(
        "invoice_amount",
        amount_ok,
        f"got {snap.get('invoice_amount') if snap else None!r}, expected {exp['invoice_amount']}",
    )

    cat = snap.get("category_index") if snap else None
    field(
        "category",
        C.is_int(cat)
        and cat == exp["category_index"]
        and snap.get("category_title") == exp["category_title"],
        f"got {cat!r}/{snap.get('category_title') if snap else None!r}, "
        f"expected {exp['category_index']}/{exp['category_title']!r}",
    )

    prio = snap.get("priority") if snap else None
    field("priority", prio == exp["priority"], f"got {prio!r}, expected {exp['priority']!r}")

    notify = snap.get("notify") if snap else None
    field("notify", notify is exp["notify"], f"got {notify!r}, expected {exp['notify']!r}")

    qty = snap.get("quantity") if snap else None
    field(
        "quantity",
        C.is_int(qty) and qty == exp["quantity"],
        f"got {qty!r}, expected {exp['quantity']}",
    )

    notes = C.normalize_text(snap.get("notes")) if snap else None
    field("notes", notes == exp["notes"], f"got {notes!r}, expected {exp['notes']!r}")

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
        f"{len(submits)} submit event(s), state submit_count={state.get('submit_count')!r}, "
        f"status={state.get('status')!r}",
    )

    checks.add("ui_events", *_ui_events(snap, submits, events))


def _ui_events(
    snap: dict[str, Any] | None, submits: list[dict[str, Any]], events: list[dict[str, Any]]
):
    """The submitted values must be explained by logged UI events."""
    weight = 0.15
    if snap is None or not submits:
        return False, weight, "no submission snapshot or no submit event"
    last = submits[-1]
    if C.details(last).get("fields") != snap:
        return False, weight, "submit event fields differ from the state submission snapshot"
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
                weight,
                f"{event_field}: submitted {snap.get(snap_key)!r} but UI events leave {replay[event_field]!r}",
            )
    return True, weight, "submitted values match the logged field edits"


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
