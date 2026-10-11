#!/usr/bin/env python3
"""Evaluate IR-04: a late cookie-consent overlay on a web page (BenchLab irconsent, WKWebView).

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402
import interrupts as I  # noqa: E402

MODE = "irconsent"


def evaluate(seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks) -> None:
    exp = C.derive_irconsent(seed)
    answers = [C.details(e) for e in C.events_of(events, "consent_answer")]
    refused = bool(answers) and all(
        a.get("analytics") is False and a.get("ads") is False for a in answers
    )
    checks.add(
        "consent_refused",
        refused,
        0.3,
        f"consent answers {[a.get('answer') for a in answers]!r}",
    )
    submits = C.events_of(events, "submit")
    last = C.details(submits[-1]) if submits else {}
    values_ok = (
        bool(submits)
        and I.norm(last.get("email")).lower() == exp["email"]
        and last.get("plan") == exp["plan"]
        and last.get("digest") is exp["digest"]
    )
    checks.add(
        "submitted_values",
        values_ok,
        0.45,
        f"last submit {last!r}; expected {exp['email']!r}, plan {exp['plan']!r}, digest {exp['digest']}",
    )
    checks.add("submitted_once", len(submits) == 1, 0.1, f"{len(submits)} submit(s)")
    checks.add(
        "state_matches_events",
        state.get("submit_count") == len(submits) and len(state.get("consent_answers") or []) == len(answers),
        0.1,
        f"state submit_count {state.get('submit_count')!r}",
    )
    shown = C.events_of(events, "consent_shown")
    checks.diag("submits_blocked", state.get("submits_blocked"), "Sign up pressed before the consent answer")
    I.record(
        checks,
        kind="consent_overlay",
        shown=bool(shown),
        exercised=bool(shown) and (not submits or shown[0]["seq"] < submits[0]["seq"]),
        handled=refused if shown else None,
        completed=values_ok and len(submits) == 1,
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
