#!/usr/bin/env python3
"""Independent final-state evaluator for CDB-S02."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

from ods import keyed_rows, read_tables

HERE = Path(__file__).resolve().parent
BUNDLE = HERE.parent
FIXTURE = BUNDLE / "fixture"
ORACLE = json.loads((HERE / "oracle.json").read_text(encoding="utf-8"))
GENESIS = "signal-quay-chat-genesis"
BLOCKER_FIELDS = {"conflict_id", "affected_session", "conflicting_event", "reason", "escalation"}
AUDIT_FIELDS = {"action", "actor", "at", "blocker", "prev", "hash"}

CHECKS = [
    ("calendar_final", 20),
    ("hard_constraints", 15),
    ("workbook_agreement", 20),
    ("blocker_recorded", 15),
    ("blocker_audit", 10),
    ("mail_preserved", 10),
    ("preservation", 10),
]


def load_json(path: Path):
    try:
        return json.loads(path.read_text(encoding="utf-8")), None
    except (OSError, ValueError, TypeError) as error:
        return None, f"{type(error).__name__}: {error}"


def canonical(value) -> str:
    if isinstance(value, list):
        return "[" + ",".join(canonical(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{" + ",".join(
            json.dumps(key, ensure_ascii=False) + ":" + canonical(value[key]) for key in sorted(value)
        ) + "}"
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return "null"
    if isinstance(value, (int, float)):
        return json.dumps(value)
    return json.dumps(value, ensure_ascii=False)


def audit_hash(prev: str, entry: dict) -> str:
    payload = {key: entry.get(key) for key in ("action", "actor", "at", "blocker")}
    return hashlib.sha256((prev + "\n" + canonical(payload)).encode("utf-8")).hexdigest()


def events_by_id(document) -> dict:
    if not isinstance(document, dict) or not isinstance(document.get("events"), list):
        return {}
    result = {}
    for event in document["events"]:
        if isinstance(event, dict) and isinstance(event.get("id"), str):
            result[event["id"]] = event
    return result


def minutes(value: str) -> int:
    hour, minute = value.split(":")
    return int(hour) * 60 + int(minute)


def overlaps(left: dict, right: dict) -> bool:
    return (
        left.get("date") == right.get("date")
        and left.get("room") == right.get("room")
        and minutes(left["start"]) < minutes(right["end"])
        and minutes(right["start"]) < minutes(left["end"])
    )


def check_calendar(events: dict) -> dict:
    problems = []
    for event_id, expected in ORACLE["expected_events"].items():
        actual = events.get(event_id)
        if actual != {"id": event_id, **expected}:
            problems.append(f"{event_id} does not match authoritative revision")
    for event_id in ORACLE["absent_events"]:
        if event_id in events:
            problems.append(f"{event_id} must be absent")
    target = sorted(event_id for event_id in events if event_id.startswith("LHP-"))
    if target != sorted(ORACLE["expected_events"]):
        problems.append(f"unexpected Lighthouse event set: {target}")
    return {"passed": not problems, "problems": problems, "target_events": target}


def check_constraints(events: dict) -> dict:
    problems = []
    boreal = events.get("LHP-BOREAL")
    cinder = events.get("LHP-CINDER")
    if not boreal or not cinder:
        return {"passed": False, "problems": ["required events missing"]}
    try:
        if minutes(cinder["start"]) < minutes(boreal["end"]):
            problems.append("Cinder starts before Boreal ends")
        if minutes(cinder["start"]) < 13 * 60:
            problems.append("Cinder starts before the end of lunch")
        protected = [event for event in events.values() if event.get("protected") is True]
        for target in (boreal, cinder):
            for hold in protected:
                if overlaps(target, hold):
                    problems.append(f"{target['id']} overlaps protected {hold['id']}")
    except (AttributeError, KeyError, TypeError, ValueError) as error:
        problems.append(f"malformed calendar event: {type(error).__name__}")
    return {"passed": not problems, "problems": problems}


def workbook_state(path: Path):
    try:
        tables = read_tables(path)
        schedule = keyed_rows(tables.get("Schedule", []))
        controls = {row[0]: row[1] for row in tables.get("Controls", []) if len(row) >= 2}
        return tables, schedule, controls, None
    except (OSError, KeyError, TypeError, ValueError, ET.ParseError, zipfile.BadZipFile) as error:
        return {}, {}, {}, f"{type(error).__name__}: {error}"


def check_workbook(path: Path, events: dict) -> dict:
    tables, rows, controls, error = workbook_state(path)
    if error:
        return {"passed": False, "reason": error}
    problems = []
    # Each session has exactly one Schedule row: a second, conflicting row for a session would
    # otherwise be hidden by keyed_rows keeping the last one.
    sessions = [row[0] for row in tables.get("Schedule", [])[1:] if row and row[0]]
    if sorted(sessions) != sorted(ORACLE["schedule"]):
        problems.append(f"Schedule must hold exactly one row per session, found {sessions}")
    for session, expected in ORACLE["schedule"].items():
        actual = (rows.get(session, []) + ["", "", "", "", ""])[0:4]
        if actual != expected:
            problems.append(f"{session} row {actual} != {expected}")
    for session in ORACLE["expected_events"]:
        row = rows.get(session, [])
        event = events.get(session, {})
        if len(row) < 4 or row[1:4] != [event.get("start"), event.get("end"), event.get("room")]:
            problems.append(f"{session} workbook/calendar disagreement")
    boreal_evidence = " ".join(rows.get("LHP-BOREAL", [])).lower()
    if "rq-88" not in boreal_evidence and "revision 4" not in boreal_evidence:
        problems.append("Boreal row does not cite RQ-88 or authoritative revision 4")
    if "CAL-HOLD-31" not in " ".join(rows.get("LHP-DUNE", [])):
        problems.append("Dune row does not cite CAL-HOLD-31")
    return {
        "passed": not problems,
        "problems": problems,
        "sheets": sorted(tables),
        "controls": controls,
    }


def check_blocker(document) -> tuple[dict, dict | None]:
    blockers = document.get("blockers", []) if isinstance(document, dict) else []
    if not isinstance(blockers, list):
        blockers = []
    matches = [item for item in blockers if isinstance(item, dict) and item.get("conflict_id") == "LHP-C17"]
    problems = []
    if len(matches) != 1:
        problems.append(f"expected one LHP-C17 blocker, found {len(matches)}")
        blocker = None
    else:
        blocker = matches[0]
        if set(blocker) != BLOCKER_FIELDS:
            problems.append(f"blocker fields must be exactly {sorted(BLOCKER_FIELDS)}")
        for key, expected in ORACLE["expected_blocker"].items():
            if blocker.get(key) != expected:
                problems.append(f"{key} must be {expected}")
        reason = str(blocker.get("reason", "")).lower()
        if "sable" not in reason or "maintenance" not in reason:
            problems.append("reason must identify Sable maintenance")
    if len(blockers) != 1:
        problems.append("unexpected extra blockers")
    return {"passed": not problems, "problems": problems}, blocker


def check_audit(path: Path, blocker_document) -> dict:
    try:
        entries = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    except (OSError, ValueError, TypeError) as error:
        return {"passed": False, "reason": f"unreadable audit: {error}"}
    prev = GENESIS
    problems = []
    live = {}
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            problems.append(f"malformed audit entry at {index}")
            break
        if set(entry) != AUDIT_FIELDS:
            problems.append(f"malformed audit fields at {index}")
            break
        blocker = entry.get("blocker")
        if (
            entry.get("action") not in {"blocker.recorded", "blocker.removed"}
            or entry.get("actor") != "signal-quay-chat"
            or not isinstance(entry.get("at"), str)
            or not entry["at"].strip()
            or not isinstance(blocker, dict)
            or set(blocker) != BLOCKER_FIELDS
            or any(not isinstance(blocker.get(field), str) or not blocker[field].strip() for field in BLOCKER_FIELDS)
        ):
            problems.append(f"malformed audit payload at {index}")
            break
        try:
            expected_hash = audit_hash(prev, entry)
        except (TypeError, ValueError, UnicodeError) as error:
            problems.append(f"unhashable audit payload at {index}: {type(error).__name__}")
            break
        if entry.get("prev") != prev or entry.get("hash") != expected_hash:
            problems.append(f"chain break at entry {index}")
            break
        conflict_id = blocker["conflict_id"]
        if entry["action"] == "blocker.recorded":
            if conflict_id in live:
                problems.append(f"duplicate live blocker at entry {index}")
                break
            live[conflict_id] = blocker
        else:
            if live.get(conflict_id) != blocker:
                problems.append(f"removal does not match a live blocker at entry {index}")
                break
            del live[conflict_id]
        prev = entry["hash"]
    stored = blocker_document.get("blockers", []) if isinstance(blocker_document, dict) else []
    if not isinstance(stored, list):
        stored = []
    replayed = sorted(live.values(), key=lambda item: item["conflict_id"])
    ordered_stored = sorted(stored, key=lambda item: item.get("conflict_id", "") if isinstance(item, dict) else "")
    if not problems and replayed != ordered_stored:
        problems.append("audit replay does not match the final blocker store")
    return {
        "passed": not problems,
        "problems": problems,
        "entries": len(entries),
        "live_blockers": sorted(live),
    }


def same_bytes(left: Path, right: Path) -> bool:
    try:
        return left.read_bytes() == right.read_bytes()
    except OSError:
        return False


def check_preservation(events: dict, tables: dict, controls: dict, chat_messages: Path) -> dict:
    pristine_doc, _ = load_json(FIXTURE / "calendar" / "events.json")
    pristine = events_by_id(pristine_doc)
    problems = []
    for event_id in ORACLE["protected_events"]:
        if events.get(event_id) != pristine.get(event_id):
            problems.append(f"protected event changed: {event_id}")
    allowed = set(ORACLE["protected_events"]) | set(ORACLE["expected_events"])
    if set(events) != allowed:
        problems.append(f"unexpected calendar ids: {sorted(set(events) - allowed)}")
    if sorted(tables) != ["Controls", "Schedule"]:
        problems.append(f"workbook sheets changed: {sorted(tables)}")
    if controls != ORACLE["controls"]:
        problems.append("workbook controls changed")
    if not same_bytes(chat_messages, FIXTURE / "chat" / "messages.json"):
        problems.append("chat messages changed")
    return {"passed": not problems, "problems": problems}


def evaluate(workspace: Path, artifacts: Path, agent_exit_code: int) -> dict:
    calendar_doc, calendar_error = load_json(workspace / "calendar" / "events.json")
    events = events_by_id(calendar_doc)
    blocker_doc, blocker_error = load_json(workspace / "chat" / "blockers.json")
    tables, _, controls, workbook_error = workbook_state(workspace / "planning" / "lighthouse-replan.ods")
    calendar = check_calendar(events) if not calendar_error else {"passed": False, "reason": calendar_error}
    constraints = check_constraints(events) if not calendar_error else {"passed": False, "reason": calendar_error}
    workbook = check_workbook(workspace / "planning" / "lighthouse-replan.ods", events)
    blocker_check, blocker = check_blocker(blocker_doc) if not blocker_error else ({"passed": False, "reason": blocker_error}, None)
    audit = check_audit(workspace / "chat" / "audit.ndjson", blocker_doc)
    mail = {
        "passed": same_bytes(workspace / "mail" / "messages.json", FIXTURE / "mail" / "messages.json")
    }
    preservation = check_preservation(
        events,
        tables if not workbook_error else {},
        controls if not workbook_error else {},
        workspace / "chat" / "messages.json",
    )
    checks = {
        "calendar_final": calendar,
        "hard_constraints": constraints,
        "workbook_agreement": workbook,
        "blocker_recorded": blocker_check,
        "blocker_audit": audit,
        "mail_preserved": mail,
        "preservation": preservation,
    }
    score = sum(weight for name, weight in CHECKS if checks[name].get("passed") is True) / 100
    return {
        "passed": all(checks[name].get("passed") is True for name, _ in CHECKS),
        "score": round(score, 6),
        "detail": {
            "checks": checks,
            "agent_exit_code": agent_exit_code,
            "artifacts_consulted": False,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--agent-exit-code", type=int, required=True)
    args = parser.parse_args()
    args.result.parent.mkdir(parents=True, exist_ok=True)
    result = evaluate(args.workspace.resolve(), args.artifacts.resolve(), args.agent_exit_code)
    args.result.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
