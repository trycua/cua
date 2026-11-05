"""Evaluate MA-04: the TextEdit note lists the unfilled, uncancelled shifts in order; the rota unchanged."""

from __future__ import annotations

import re
import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, rtf, web  # noqa: E402

TASK_ID = "MA-04"
NOTE = "summary.rtf"
EXPECTED = [
    ("monday", "13:00", "16:00", "packing"),
    ("tuesday", "09:30", "13:00", "front desk"),
    ("wednesday", "09:00", "12:00", "warehouse sorting"),
    ("wednesday", "14:00", "17:00", "delivery driver"),
    ("thursday", "17:00", "19:30", "front desk"),
    ("friday", "13:00", "16:00", "packing"),
    ("saturday", "10:00", "13:00", "front desk"),
]
LINE = re.compile(r"([a-z]+)\s+(\d{1,2}:\d{2})\s*[-‐‑‒–—]\s*(\d{1,2}:\d{2})\s+(.+?)\.?")


def clock(value: str) -> str:
    h, m = value.split(":")
    return f"{int(h):02d}:{m}"


def parse(line: str) -> tuple[str, str, str, str] | None:
    m = LINE.fullmatch(web.norm(line).casefold())
    if not m:
        return None
    return (m.group(1), clock(m.group(2)), clock(m.group(3)), web.norm(m.group(4)))


def evaluate(workspace: Path, checks: common.Checks) -> None:
    try:
        lines = [p.text for p in rtf.load(workspace / NOTE) if p.text.strip()]
        detail = ""
    except OSError as error:
        lines, detail = [], f"cannot read the note: {error}"
    parsed = [parse(line) for line in lines]
    shown = [web.norm(line) for line in lines][:12]
    missing = [e for e in EXPECTED if e not in parsed]
    checks.add("every_open_shift_listed", bool(lines) and not missing, detail or f"missing {missing}; note {shown}")
    extra = [lines[i] for i, p in enumerate(parsed) if p is None or p not in EXPECTED]
    dupes = len(parsed) != len(set(parsed))
    checks.add("nothing_else_in_note", bool(lines) and not extra and not dupes, detail or f"unexpected lines {extra}; duplicates {dupes}")
    checks.add("chronological_order", parsed == EXPECTED, detail or f"note {shown}")

    state, events = web.load(workspace)
    initial = common.read_json(BUNDLE / "app" / "initial-state.json")
    unchanged = not state or state == initial
    checks.add("rota_unchanged", unchanged, "rota as it was" if unchanged else "the rota was edited", kind="preservation")
    checks.diag("assign_events", [e.get("id") for e in web.of_type(events, "shift_assigned")])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
