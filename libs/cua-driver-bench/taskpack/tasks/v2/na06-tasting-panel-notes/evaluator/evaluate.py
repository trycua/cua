"""Evaluate NA-06: Round 3 corrected and extended; a new Round 4 shortlist note in the same folder."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common  # noqa: E402

TASK_ID = "NA-06"
STATE = common.read_json(BUNDLE / "reset" / "app_state.json")
FOLDER = "Tasting panel"
TRASH = "recently deleted"  # Notes keeps deleted notes (also those of earlier trials' folders) there
ROUND3 = ["Round 3 scores", "Apple & ginger: 7", "Pear & cardamom: 8", "Plum & star anise: 9", "Quince & bay: 8"]
ROUND4 = ["Round 4 shortlist", "Pear & cardamom", "Plum & star anise"]


def lines(text: object) -> list[str]:
    out = []
    for line in str(text or "").replace(" ", "\n").splitlines():
        line = " ".join(line.replace(" ", " ").split())
        if line:
            out.append(line)
    return out


def norm(s: object) -> str:
    return " ".join(str(s or "").split()).lower()


def same(a: list[str], b: list[str]) -> bool:
    return [x.lower().rstrip(".") for x in a] == [x.lower().rstrip(".") for x in b]


def evaluate(workspace: Path, checks: common.Checks) -> None:
    data = common.read_json(workspace / "export" / "notes.json")
    if not isinstance(data, dict):
        checks.add("exported", False, "no notes export")
        return
    notes = [n for n in data.get("notes", []) if isinstance(n, dict) and norm(n.get("folder")) != TRASH]
    ours = [n for n in notes if n.get("folder") == FOLDER]
    by_first = {}
    for n in ours:
        first = (lines(n.get("plaintext")) or [""])[0].lower()
        by_first.setdefault(first, []).append(n)
    r3 = by_first.get("round 3 scores", [])
    r3_lines = lines(r3[0].get("plaintext")) if len(r3) == 1 else []
    checks.add("plum_score_changed", "plum & star anise: 9" in [l.lower() for l in r3_lines] and
               "plum & star anise: 6" not in [l.lower() for l in r3_lines], repr(r3_lines))
    checks.add("quince_line_added_last", bool(r3_lines) and r3_lines[-1].lower() == "quince & bay: 8", repr(r3_lines))
    checks.add("round3_exact", same(r3_lines, ROUND3), repr(r3_lines))
    r4 = [n for n in notes if (lines(n.get("plaintext")) or [""])[0].lower() == "round 4 shortlist"]
    checks.add("round4_in_folder_once", len(r4) == 1 and r4[0].get("folder") == FOLDER,
               f"{[(n.get('folder'), n.get('name')) for n in r4]}")
    r4_lines = lines(r4[0].get("plaintext")) if len(r4) == 1 else []
    checks.add("round4_lines", same(r4_lines, ROUND4), repr(r4_lines))
    panel = by_first.get("panel members", [])
    checks.add("panel_note_unchanged", len(panel) == 1 and same(lines(panel[0].get("plaintext")), STATE["lines"]["Panel members"]),
               repr([lines(n.get("plaintext")) for n in panel]), kind="preservation")
    checks.add("no_other_notes", len(ours) == 3 and data.get("folders", []).count(FOLDER) == 1,
               f"{len(ours)} notes in the folder", kind="preservation")


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
