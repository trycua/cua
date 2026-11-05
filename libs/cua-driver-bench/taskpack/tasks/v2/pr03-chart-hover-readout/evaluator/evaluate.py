"""Evaluate PR-03: one submitted answer naming week 38 with 418 returns, for Sallowmead North."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, web  # noqa: E402

TASK_ID = "PR-03"
WEEK = "W38"
RETURNS = 418
DEPOT = "Sallowmead North"


def evaluate(workspace: Path, checks: common.Checks) -> None:
    state, events = web.load(workspace)
    subs = [s for s in state.get("submissions") or [] if isinstance(s, dict)]
    submitted = web.of_type(events, "answer_submitted")
    last = subs[-1] if subs else {}
    checks.add("week_ok", last.get("week") == WEEK, f"week={last.get('week')!r}")
    value = last.get("returns")
    checks.add("returns_ok", isinstance(value, int) and not isinstance(value, bool) and value == RETURNS, f"returns={value!r}")
    checks.add("submitted_once", len(submitted) == 1 and len(subs) == 1, f"{len(submitted)} submission event(s), {len(subs)} saved")
    checks.add("depot_kept", all(s.get("depot") == DEPOT for s in subs), f"depots={[s.get('depot') for s in subs]}", kind="preservation")
    checks.diag("submit_trusted", [e.get("trusted") for e in submitted])
    checks.diag("tooltips_shown", _tooltips(workspace)[:40])



def _tooltips(workspace: Path) -> list:
    return [(e.get("series"), e.get("week")) for e in web.of_type(web.load(workspace)[1], "tooltip_shown")]


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
