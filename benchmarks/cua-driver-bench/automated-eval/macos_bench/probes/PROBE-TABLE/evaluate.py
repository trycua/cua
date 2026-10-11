#!/usr/bin/env python3
"""Evaluate PROBE-TABLE: BenchLab table mode.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "table"
CODE_RE = re.compile(r"K-(\d{4})")


def _codes(value: Any) -> list[str] | None:
    if isinstance(value, list) and all(isinstance(v, str) for v in value):
        return value
    return None


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    expected = set(C.derive_table(seed)["codes"])
    saves = C.events_of(events, "save")
    saved = _codes(state.get("saved_flagged"))
    live = _codes(state.get("flagged"))

    checks.add(
        "save_pressed",
        state.get("saved") is True
        and bool(saves)
        and state.get("save_count") == len(saves)
        and saved is not None,
        0.15,
        f"{len(saves)} save event(s), state save_count={state.get('save_count')!r}",
    )

    if saved is None:
        checks.add("required_flagged", False, 0.25, "nothing was saved")
        checks.add("no_extra_flagged", False, 0.2, "nothing was saved")
    else:
        missing = sorted(expected - set(saved))
        extra = sorted(set(saved) - expected)
        checks.add(
            "required_flagged",
            not missing,
            0.25,
            f"missing {missing}" if missing else "all target codes saved",
        )
        checks.add(
            "no_extra_flagged",
            not extra and len(saved) == len(set(saved)),
            0.2,
            f"extra {extra}" if extra else "no other codes saved",
        )

    checks.add(
        "final_state_matches",
        live is not None and saved is not None and set(live) == expected and set(saved) == expected,
        0.1,
        f"live flagged {live!r}, saved {saved!r}",
    )
    checks.add("ui_events", *_ui_events(state, events, saves, saved))


def _ui_events(
    state: dict[str, Any],
    events: list[dict[str, Any]],
    saves: list[dict[str, Any]],
    saved: list[str] | None,
):
    """Replaying the logged flag toggles must reproduce the state and the saved snapshot."""
    weight = 0.25
    if not saves or saved is None:
        return False, weight, "no save event"
    flagged: set[str] = set()
    at_last_save: set[str] | None = None
    last_save_seq = saves[-1]["seq"]
    for event in events:
        d = C.details(event)
        if event.get("type") == "flag_toggle":
            code = d.get("code")
            if (
                not isinstance(code, str)
                or not CODE_RE.fullmatch(code)
                or not isinstance(d.get("flagged"), bool)
            ):
                return False, weight, f"malformed flag_toggle event at seq {event.get('seq')}"
            (flagged.add if d["flagged"] else flagged.discard)(code)
        if event["seq"] == last_save_seq:
            at_last_save = set(flagged)
    live = _codes(state.get("flagged"))
    if live is None or set(live) != flagged:
        return (
            False,
            weight,
            f"state flagged {live!r} is not explained by toggle events {sorted(flagged)}",
        )
    save_details = C.details(saves[-1])
    if _codes(save_details.get("flagged")) is None or set(save_details["flagged"]) != set(saved):
        return False, weight, "save event flagged list differs from state saved_flagged"
    if at_last_save != set(saved):
        return (
            False,
            weight,
            f"toggles up to the save give {sorted(at_last_save or [])}, state saved {sorted(saved)}",
        )
    return True, weight, "flag toggles and save snapshot are consistent"


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
