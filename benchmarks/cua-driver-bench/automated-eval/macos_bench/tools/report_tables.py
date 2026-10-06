#!/usr/bin/env python3
"""Per-task, per-arm tables for the report from a run's results.jsonl (final, non-smoke, non-excluded rows).

  report_tables.py RESULTS.jsonl --tasks CDB-S01 CDB-S02 ...   (markdown on stdout)
"""

from __future__ import annotations

import argparse
import json
import statistics as st
import sys
from collections import defaultdict
from pathlib import Path

ARMS = ("cc-cua-driver", "cc-codex-cu")
LABEL = {"cc-cua-driver": "A Cua Driver", "cc-codex-cu": "B Codex CU"}


def load(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text("utf-8").splitlines():
        if line.strip():
            d = json.loads(line)
            if not d.get("smoke") and d.get("final", True) and not d.get("excluded"):
                rows.append(d)
    return rows


def ms(values: list[float], digits: int = 2) -> str:
    if not values:
        return "n/a"
    sd = st.pstdev(values) if len(values) > 1 else 0.0
    return f"{st.mean(values):.{digits}f} ({sd:.{digits}f})"


def gui_calls(row: dict) -> int:
    return sum(v for k, v in row["tool_calls"]["by_name"].items() if k.startswith("mcp__"))


def tokens(row: dict) -> dict:
    return row.get("tokens") or {}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("results", type=Path)
    ap.add_argument("--tasks", nargs="*")
    ap.add_argument("--tokens", action="store_true", help="token table instead of the main table")
    args = ap.parse_args()
    rows = load(args.results)
    tasks = args.tasks or sorted({r["task"] for r in rows})
    by = defaultdict(list)
    for r in rows:
        by[(r["task"], r["arm"])].append(r)
    if args.tokens:
        print("| Task | Arm | n | Input (uncached) | Output | Cache read | Cache write | Equivalent cost USD |")
        print("|---|---|---|---|---|---|---|---|")
        for t in tasks:
            for arm in ARMS:
                v = by.get((t, arm), [])
                if v:
                    g = lambda k: ms([tokens(r).get(k, 0) for r in v], 0)  # noqa: E731
                    print(f"| {t} | {LABEL[arm]} | {len(v)} | {g('input')} | {g('output')} | {g('cache_read')} | {g('cache_write')} | {ms([r['cost_usd'] or 0 for r in v], 3)} |")
        return 0
    print("| Task | Arm | n | Success | Score mean (sd) | Wall s mean (sd) | Turns | Computer-use calls | Failed calls | Output tokens | Cache-read tokens | Cost USD | Pointer moved | Focus stolen | Peeks | Flagged |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for t in tasks:
        for arm in ARMS:
            v = by.get((t, arm), [])
            if not v:
                continue
            n = len(v)
            ok = sum(1 for r in v if r["passed"])
            fails = sum((r["tool_calls"].get("failed") or 0) for r in v)
            moved = sum(1 for r in v if r["disturbance"].get("pointer_moved"))
            front = sum(1 for r in v if r["disturbance"].get("frontmost_changed"))
            peeks = sum(1 for r in v if r.get("evaluator_peeks"))
            flagged = sum(1 for r in v if r.get("side_door_flag"))
            print(
                f"| {t} | {LABEL[arm]} | {n} | {ok}/{n}, mean {ok / n:.2f}, var {ok / n * (1 - ok / n):.2f} | {ms([r['score'] or 0.0 for r in v])} | {ms([r['wall_s'] for r in v], 0)} | {ms([r['turns'] or 0 for r in v], 1)} | {ms([gui_calls(r) for r in v], 1)} | {fails} | {ms([tokens(r).get('output', 0) for r in v], 0)} | {ms([tokens(r).get('cache_read', 0) for r in v], 0)} | {ms([r['cost_usd'] or 0 for r in v], 3)} | {moved}/{n} | {front}/{n} | {peeks} | {flagged} |"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
