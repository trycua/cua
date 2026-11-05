"""Evaluate NA-04: constitution locked, March minutes commented, draft agenda renamed; nothing else changed."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, finder  # noqa: E402

TASK_ID = "NA-04"
META = common.read_json(BUNDLE / "reset" / "meta.json")
COMMENT = "approved at the april meeting"
RENAMES = {"draft agenda.txt": "agenda-may.txt"}


def evaluate(workspace: Path, checks: common.Checks) -> None:
    folder = workspace / META["folder"]
    fixture = BUNDLE / "fixture" / META["folder"]
    present = {p.name for p in folder.iterdir() if p.name != ".DS_Store"} if folder.is_dir() else set()
    const = folder / "constitution-2024.txt"
    checks.add("constitution_locked", const.exists() and finder.locked(const), "locked" if const.exists() and finder.locked(const) else "not locked")
    minutes = folder / "minutes-march.txt"
    text = " ".join(finder.comment(minutes).split()).lower().rstrip(".") if minutes.exists() else ""
    checks.add("minutes_comment", text == COMMENT, repr(finder.comment(minutes) if minutes.exists() else None))
    new = folder / "agenda-may.txt"
    checks.add("agenda_renamed", new.exists() and "draft agenda.txt" not in present
               and new.read_bytes() == (fixture / "draft agenda.txt").read_bytes(), f"files {sorted(present)}")
    want_names = {RENAMES.get(p.name, p.name) for p in fixture.iterdir()}
    same = present == want_names and all(
        (folder / RENAMES.get(p.name, p.name)).read_bytes() == p.read_bytes() for p in fixture.iterdir())
    checks.add("contents_unchanged", same, f"extra {sorted(present - want_names)}, missing {sorted(want_names - present)}", kind="preservation")
    others_ok = True
    detail = []
    for p in fixture.iterdir():
        name = RENAMES.get(p.name, p.name)
        if name not in present or name in ("constitution-2024.txt", "minutes-march.txt"):
            continue
        if finder.locked(folder / name) != (p.name in META["locked"]):
            others_ok = False
            detail.append(f"{name} locked={finder.locked(folder / name)}")
        if finder.comment(folder / name) != META["comments"].get(p.name, ""):
            others_ok = False
            detail.append(f"{name} comment={finder.comment(folder / name)!r}")
    checks.add("other_info_unchanged", others_ok, "; ".join(detail), kind="preservation")


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
