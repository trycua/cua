"""Scripted solution for NA-01, used only by `_kit/devtools/vm_validate.py` in a macOS VM.

It changes the app's data or files directly (not through the GUI), to prove the evaluator passes a solved state.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_kit"))

from v2kit import mac  # noqa: E402

SCRIPT = r"""
const app = Application("Calendar");
const cal = app.calendars.byName("Youth Theatre 2026");
const ev = cal.events().find(e => e.summary() === "Rehearsal: full run");
ev.endDate = parseLocal("2026-11-18T21:00"); ev.startDate = parseLocal("2026-11-18T18:30");
const n = app.Event({summary: "Lighting check", startDate: parseLocal("2026-11-28T14:30"), endDate: parseLocal("2026-11-28T16:00"), location: "Gallowfield Arts Centre"});
cal.events.push(n);
const made = cal.events().find(e => e.summary() === "Lighting check");
made.displayAlarms.push(app.DisplayAlarm({triggerInterval: -60}));
JSON.stringify(true);
"""


def solve(workspace: Path) -> None:
    mac.jxa(SCRIPT)
