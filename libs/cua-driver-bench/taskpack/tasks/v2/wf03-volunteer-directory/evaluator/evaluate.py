"""Evaluate WF-03: three volunteers moved to Food Pantry, one Maren Ostwick deactivated, nobody else changed."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, web  # noqa: E402

TASK_ID = "WF-03"
MOVE = {"V-1112": "Hesper Blackmore", "V-1229": "Teodor Quillfeather", "V-1259": "Ilse Varnham"}
TEAM = "pantry"
DEACTIVATE = "V-1214"  # Maren Ostwick, maren.ostwick@moorpost.example


def evaluate(workspace: Path, checks: common.Checks) -> None:
    state, events = web.load(workspace)
    initial = {v["id"]: v for v in common.read_json(BUNDLE / "app" / "initial-state.json")["volunteers"]}
    rows = [v for v in state.get("volunteers") or [] if isinstance(v, dict)]
    now = {v.get("id"): v for v in rows}
    for vid, name in MOVE.items():
        v = now.get(vid, {})
        checks.add(f"moved_{vid}", v.get("team") == TEAM, f"{name}: team={v.get('team')!r}")
    d = now.get(DEACTIVATE, {})
    checks.add("deactivated_right_maren", d.get("status") == "inactive", f"{DEACTIVATE}: status={d.get('status')!r}")
    checks.add("same_volunteers", len(rows) == len(now) and set(now) == set(initial), f"{len(rows)} rows", kind="preservation")
    for vid in MOVE:
        want = {**initial[vid], "team": TEAM}
        checks.add(f"{vid}_otherwise_unchanged", now.get(vid, want) == want or now.get(vid) == initial[vid], f"{now.get(vid)!r}", kind="preservation")
    want = {**initial[DEACTIVATE], "status": "inactive"}
    got = now.get(DEACTIVATE)
    checks.add(f"{DEACTIVATE}_otherwise_unchanged", got in (want, initial[DEACTIVATE]) or got is None, f"{got!r}", kind="preservation")
    twin = now.get("V-1211", {})
    checks.add("other_maren_still_active", twin.get("status") == "active", f"V-1211: status={twin.get('status')!r}", kind="preservation")
    others = [vid for vid in initial if vid not in MOVE and vid != DEACTIVATE and now.get(vid) not in (None, initial[vid])]
    checks.add("others_unchanged", not others, f"changed: {[(i, initial[i]['name']) for i in others][:10]}", kind="preservation")
    checks.diag("events", [(e.get("type"), e.get("id")) for e in events if e.get("type") not in ("state_saved", "server_started")])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
