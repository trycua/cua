"""Evaluate NA-01: the full run moved to Wednesday 18 November, a lighting check added with a 1-hour alert."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common  # noqa: E402

TASK_ID = "NA-01"
STATE = common.read_json(BUNDLE / "reset" / "app_state.json")
CAL = STATE["calendar"]
MOVED = {"summary": "Rehearsal: full run", "start": "2026-11-18T18:30", "end": "2026-11-18T21:00"}
NEW = {"summary": "Lighting check", "start": "2026-11-28T14:30", "end": "2026-11-28T16:00", "location": "Gallowfield Arts Centre"}


def norm(s: object) -> str:
    return " ".join(str(s or "").split()).lower()


def evaluate(workspace: Path, checks: common.Checks) -> None:
    data = common.read_json(workspace / "export" / "calendar.json")
    if not isinstance(data, dict):
        checks.add("exported", False, "no calendar export")
        return
    events = [e for e in data.get("events", []) if isinstance(e, dict)]
    ours = [e for e in events if e.get("calendar") == CAL]
    runs = [e for e in events if norm(e.get("summary")) == norm(MOVED["summary"])]
    run = runs[0] if len(runs) == 1 and runs[0].get("calendar") == CAL else {}
    checks.add("full_run_moved", run.get("start") == MOVED["start"] and run.get("end") == MOVED["end"],
               f"{len(runs)} full-run event(s); {run.get('start')}..{run.get('end')}")
    checks.add("full_run_location_kept", bool(run) and run.get("start") == MOVED["start"]
               and norm(run.get("location")) == norm("Orchard Street Studio"), repr(run.get("location")))
    new = [e for e in events if norm(e.get("summary")) == norm(NEW["summary"])]
    checks.add("lighting_check_once", len(new) == 1 and new[0].get("calendar") == CAL,
               f"{[(e.get('calendar'), e.get('start')) for e in new]}")
    one = new[0] if len(new) == 1 else {}
    checks.add("lighting_check_time", one.get("start") == NEW["start"] and one.get("end") == NEW["end"],
               f"{one.get('start')}..{one.get('end')}")
    checks.add("lighting_check_location", norm(one.get("location")) == norm(NEW["location"]), repr(one.get("location")))
    alarms = [a for a in one.get("alarms") or [] if isinstance(a, dict)]
    checks.add("lighting_check_alert", any(a.get("minutes") == -60 and a.get("kind") in ("display", "sound") for a in alarms),
               f"alarms {alarms!r}")
    keep = [e for e in STATE["events"] if e["summary"] != MOVED["summary"]]
    missing = [e["summary"] for e in keep if not any(
        o.get("summary") == e["summary"] and o.get("start") == e["start"] and o.get("end") == e["end"]
        and norm(o.get("location")) == norm(e["location"]) for o in ours)]
    checks.add("other_events_unchanged", not missing, f"changed or missing: {missing}", kind="preservation")
    extra = [e.get("summary") for e in ours if norm(e.get("summary")) not in
             {norm(x["summary"]) for x in STATE["events"]} | {norm(NEW["summary"])}]
    checks.add("no_extra_events", not extra and len(ours) == 6, f"{len(ours)} events; extra {extra}", kind="preservation")


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
