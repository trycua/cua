"""Checker tests for NA-01 on exported calendar states (the export itself runs only in the VM)."""

from __future__ import annotations

import copy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common  # noqa: E402
import evaluate as ev  # noqa: E402

STATE = common.read_json(BUNDLE / "reset/app_state.json")


def export(events: list[dict]) -> dict:
    out = []
    for e in events:
        out.append({"calendar": e.get("calendar", STATE["calendar"]), "summary": e["summary"], "start": e["start"], "end": e["end"],
                    "location": e.get("location", ""), "description": "", "allday": False, "recurrence": "",
                    "alarms": e.get("alarms", []), "uid": "x"})
    return {"calendars": ["Home", "Work", STATE["calendar"]], "events": out}


def solved() -> list[dict]:
    events = copy.deepcopy(STATE["events"])
    for e in events:
        if e["summary"] == "Rehearsal: full run":
            e["start"], e["end"] = "2026-11-18T18:30", "2026-11-18T21:00"
    events.append({"summary": "Lighting check", "start": "2026-11-28T14:30", "end": "2026-11-28T16:00",
                   "location": "Gallowfield Arts Centre", "alarms": [{"kind": "display", "minutes": -60}]})
    return events


class NA01(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_with(self, events: list[dict] | None) -> dict:
        if events is not None:
            common.write_json(self.ws / "export/calendar.json", export(events))
        return common.evaluate_dir("NA-01", ev.evaluate, self.ws)

    def test_no_export_fails(self) -> None:
        r = self.run_with(None)
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_with(copy.deepcopy(STATE["events"]))
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        r = self.run_with(solved())
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_duplicated_instead_of_moved_fails(self) -> None:
        events = solved() + [e for e in STATE["events"] if e["summary"] == "Rehearsal: full run"]
        self.assertFalse(self.run_with(events)["passed"])

    def test_alert_at_event_time_fails(self) -> None:
        events = solved()
        events[-1]["alarms"] = [{"kind": "display", "minutes": 0}]
        r = self.run_with(events)
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["lighting_check_alert"]["passed"])

    def test_event_in_wrong_calendar_fails(self) -> None:
        events = solved()
        events[-1]["calendar"] = "Home"
        self.assertFalse(self.run_with(events)["passed"])

    def test_other_event_changed_fails(self) -> None:
        events = solved()
        events[0]["location"] = "Somewhere else"
        r = self.run_with(events)
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)


if __name__ == "__main__":
    unittest.main()
