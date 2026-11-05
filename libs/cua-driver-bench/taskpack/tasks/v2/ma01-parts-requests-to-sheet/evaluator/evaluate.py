"""Evaluate MA-01: the approved, unlogged requests added to the Calc log in id order and marked logged on the page."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, odf, web  # noqa: E402

TASK_ID = "MA-01"
SHEET = "Purchases 2026"
HEADER = ["Request", "Part", "Qty", "Requested by"]
EXISTING = [
    ("PR-2031", "Stepper motor, NEMA 17", 2, "Odile Fenwright"),
    ("PR-2033", "Brass heat-set inserts M3 (pack of 100)", 1, "Tamsin Achterberg"),
    ("PR-2036", "Router bit set, 6 mm shank", 1, "Odile Fenwright"),
]
NEW = [
    ("PR-2035", "Silicone wire 22 AWG, red, 10 m", 2, "Imke Dalgarno"),
    ("PR-2037", "Laser tube mirror, 25 mm", 3, "Bastien Quarrie"),
    ("PR-2039", "Soldering tips, conical (set of 5)", 4, "Tamsin Achterberg"),
    ("PR-2041", "Hot-melt glue sticks, 11 mm (1 kg)", 2, "Imke Dalgarno"),
]


def text(value: str) -> str:
    return odf.norm(value).casefold().rstrip(".")


def qty_ok(cell: odf.Cell, want: int) -> bool:
    if cell.number is not None:
        return cell.number == want
    return cell.display == str(want)


def row_matches(row: list[odf.Cell], want: tuple[str, str, int, str]) -> bool:
    cells = row + [odf.EMPTY] * (4 - len(row))
    rid, part, qty, who = want
    return (
        cells[0].display.upper() == rid
        and text(cells[1].text) == text(part)
        and qty_ok(cells[2], qty)
        and text(cells[3].text) == text(who)
        and not any(c.display for c in cells[4:])
    )


def row_desc(row: list[odf.Cell]) -> list[str]:
    return [c.display for c in row]


def evaluate(workspace: Path, checks: common.Checks) -> None:
    # ---- the spreadsheet
    rows: list[list[odf.Cell]] = []
    detail = ""
    try:
        sheets = odf.sheets(odf.load(workspace / "purchase-log.fods"))
        rows = sheets.get(SHEET) or []
        if SHEET not in sheets:
            detail = f"no sheet {SHEET!r}; sheets {list(sheets)}"
    except (OSError, ValueError) as error:
        detail = f"cannot read the log: {error}"
    data = [r for r in rows[1:] if any(c.display for c in r)]
    ids = [r[0].display.upper() if r else "" for r in data]
    want_ids = [w[0] for w in EXISTING + NEW]
    checks.add("log_rows_in_order", ids == want_ids, detail or f"request column {ids}, want {want_ids}")
    for i, want in enumerate(NEW):
        idx = len(EXISTING) + i
        got = data[idx] if idx < len(data) else []
        checks.add(f"log_row_{want[0]}", row_matches(got, want), f"row {idx + 2}: {row_desc(got)}")
    header = [c.display for c in rows[0]] if rows else []
    kept = header == HEADER and all(i < len(data) and row_matches(data[i], w) for i, w in enumerate(EXISTING))
    checks.add("existing_log_rows_kept", kept, f"header {header}; first rows {[row_desc(r) for r in data[:3]]}", kind="preservation")

    # ---- the web app
    state, events = web.load(workspace)
    initial = {r["id"]: r for r in common.read_json(BUNDLE / "app" / "initial-state.json")["requests"]}
    current = {r.get("id"): r for r in state.get("requests") or [] if isinstance(r, dict)}
    for rid, *_ in NEW:
        got = current.get(rid) or {}
        checks.add(f"marked_{rid}", got.get("logged") is True, f"{rid} logged={got.get('logged')!r}")
    new_ids = {w[0] for w in NEW}
    problems = []
    if current and set(current) != set(initial):
        problems.append(f"request ids {sorted(current)}")
    for rid, before in initial.items():
        got = current.get(rid)
        if got is None:
            continue
        # the four requests may only change their logged flag (graded above); every other request stays as it was
        expect = {**before, "logged": got.get("logged")} if rid in new_ids else before
        if got != expect:
            problems.append(f"{rid} changed")
    checks.add("other_requests_unchanged", not problems, "; ".join(problems) or "ok", kind="preservation")
    checks.diag("logged_events", [e.get("id") for e in web.of_type(events, "request_logged")])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
