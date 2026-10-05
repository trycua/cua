#!/usr/bin/env python3
"""Evaluate MB-08: open Settings from the menu bar, change three settings, Apply, confirm the ticket.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json

The ticket is a number the app shows in the main window only after Apply. It is a hash of the seed
and the applied settings, so entering the right ticket proves the three settings were applied and
that the agent read the main window afterwards. Required: settings_opened, applied_name,
applied_theme, applied_compact (the last Apply event), ticket_correct, confirm_after_apply,
confirm_pressed (plus integrity). Diagnostics: how Settings was opened (menu or key equivalent),
number of Apply presses, wrong Confirm presses.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "settings"


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_settings(seed)
    opened = C.events_of(events, "settings_opened")
    applies = C.events_of(events, "apply")
    confirms = C.events_of(events, "confirm")
    la = C.details(applies[-1]) if applies else {}
    lc = C.details(confirms[-1]) if confirms else {}

    checks.add(
        "settings_opened",
        bool(opened) and state.get("settings_open_count") == len(opened),
        0.05,
        f"{len(opened)} settings_opened event(s)",
    )

    name = la.get("name").strip() if isinstance(la.get("name"), str) else None
    checks.add(
        "applied_name",
        bool(applies) and name == exp["name"],
        0.1,
        f"applied {name!r}, expected {exp['name']!r}",
    )
    checks.add(
        "applied_theme",
        bool(applies) and la.get("theme") == exp["theme"],
        0.1,
        f"applied {la.get('theme')!r}, expected {exp['theme']!r}",
    )
    checks.add(
        "applied_compact",
        bool(applies) and la.get("compact") is exp["compact_target"],
        0.1,
        f"applied compact={la.get('compact')!r}, expected {exp['compact_target']!r}",
    )

    shown = la.get("ticket")
    entered = lc.get("entered").strip() if isinstance(lc.get("entered"), str) else None
    ticket_ok = (
        bool(confirms)
        and C.is_int(shown)
        and shown == exp["ticket"]
        and entered == str(exp["ticket"])
        and lc.get("ticket_shown") == shown
    )
    checks.add(
        "ticket_correct",
        ticket_ok,
        0.3,
        f"entered {entered!r}, shown {shown!r}, expected {exp['ticket']}",
    )

    after = (
        bool(applies)
        and bool(confirms)
        and confirms[-1]["seq"] > applies[-1]["seq"]
        and lc.get("revision") == la.get("revision")
    )
    checks.add(
        "confirm_after_apply",
        after,
        0.1,
        "the last Confirm follows the last Apply" if after else "no Confirm after the last Apply",
    )

    pressed = (
        bool(confirms)
        and state.get("confirm_count") == len(confirms)
        and state.get("status") == "Confirmed"
    )
    checks.add(
        "confirm_pressed",
        pressed,
        0.15,
        f"{len(confirms)} confirm event(s), status {state.get('status')!r}",
    )

    wrong = [C.details(e).get("entered") for e in confirms[:-1]]
    checks.diag("opened_via", [C.details(e).get("via") for e in opened])
    checks.diag("apply_presses", len(applies))
    checks.diag("earlier_confirm_entries", wrong)


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
