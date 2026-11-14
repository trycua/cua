#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""One table per Cua Driver version from Bench v2 results.jsonl files.

    driver_version_table.py RUN_DIR_OR_RESULTS.jsonl [...] [--md out.md] [--json out.json]

Rows are grouped by the driver version recorded in each trial (``cua_driver_version``, e.g. ``cua-driver 0.41.3``).
Use ``--label PATH=NAME`` to name a run's driver yourself (needed when two runs report the same version string).
Per version and condition (foreground / background) it prints: trials, pass rate, background completion (task
passed and the user was not disturbed, the A22 rule), mean and median wall time, mean tokens (input, output, cache
read, cache write), mean cost in USD, mean tool calls, the share of tool calls that were batch calls
(``run_steps`` / ``run_script``) and the share of driver actions that rode in a batch.

Smoke rows, retried attempts and excluded (infrastructure) trials are dropped, as in analyze_v2.py.
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any


def load(path: Path, label: str | None) -> list[dict[str, Any]]:
    file = path / "results.jsonl" if path.is_dir() else path
    rows = []
    for line in file.read_text("utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        if row.get("smoke") or not row.get("final", True) or row.get("excluded"):
            continue
        row["_driver"] = label or str(row.get("cua_driver_version") or row.get("arm") or "?")
        row["_src"] = str(file)
        rows.append(row)
    return rows


def mean(xs: list[float]) -> float | None:
    xs = [x for x in xs if x is not None]
    return sum(xs) / len(xs) if xs else None


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    n = len(rows)
    passed = sum(1 for r in rows if r.get("passed"))
    bg = [r for r in rows if r.get("condition") == "background"]
    bgc = sum(1 for r in bg if (r.get("multiplayer") or {}).get("background_completion"))
    tok = lambda k: mean([(r.get("tokens") or {}).get(k, 0) for r in rows])  # noqa: E731
    calls = [(r.get("tool_calls") or {}).get("total", 0) for r in rows]
    batch = [r.get("batching") for r in rows if r.get("batching")]
    batch_calls = sum(b["batch_calls"] for b in batch)
    mcp_calls = sum(b["mcp_calls"] for b in batch)
    actions = sum(b["actions_total"] for b in batch)
    inner = sum(b["steps_in_run_steps"] + b["script_inner_calls_est"] for b in batch)
    walls = [r["wall_s"] for r in rows if r.get("wall_s") is not None]
    return {
        "trials": n,
        "passed": passed,
        "pass_rate": passed / n if n else None,
        "background_trials": len(bg),
        "background_completion": bgc,
        "background_completion_rate": bgc / len(bg) if bg else None,
        "wall_s_mean": mean(walls),
        "wall_s_median": statistics.median(walls) if walls else None,
        "tokens_input_mean": tok("input"),
        "tokens_output_mean": tok("output"),
        "tokens_cache_read_mean": tok("cache_read"),
        "tokens_cache_write_mean": tok("cache_write"),
        "cost_usd_mean": mean([r.get("cost_usd") for r in rows if r.get("cost_usd") is not None]),
        "cost_usd_total": sum(r.get("cost_usd") or 0 for r in rows),
        "tool_calls_mean": mean(calls),
        "batch_call_share": batch_calls / mcp_calls if mcp_calls else None,
        "batched_action_share": inner / actions if actions else None,
        "trials_with_batching_data": len(batch),
    }


def f(x: Any, digits: int = 1, pct: bool = False) -> str:
    if x is None:
        return "-"
    return f"{x * 100:.0f}%" if pct else f"{x:,.{digits}f}"


COLS = [
    ("trials", "n", lambda s: str(s["trials"])),
    ("pass", "pass", lambda s: f"{f(s['pass_rate'], pct=True)} ({s['passed']}/{s['trials']})"),
    (
        "bg",
        "bg completion",
        lambda s: f"{f(s['background_completion_rate'], pct=True)} ({s['background_completion']}/{s['background_trials']})"
        if s["background_trials"]
        else "-",
    ),
    ("wall", "wall s mean / median", lambda s: f"{f(s['wall_s_mean'])} / {f(s['wall_s_median'])}"),
    ("in", "input tok", lambda s: f(s["tokens_input_mean"], 0)),
    ("out", "output tok", lambda s: f(s["tokens_output_mean"], 0)),
    ("cr", "cache read", lambda s: f(s["tokens_cache_read_mean"], 0)),
    ("cw", "cache write", lambda s: f(s["tokens_cache_write_mean"], 0)),
    ("cost", "$ / trial", lambda s: f(s["cost_usd_mean"], 3)),
    ("calls", "tool calls", lambda s: f(s["tool_calls_mean"])),
    ("bc", "batch calls", lambda s: f(s["batch_call_share"], pct=True)),
    ("ba", "batched actions", lambda s: f(s["batched_action_share"], pct=True)),
]


def table(groups: dict[tuple[str, str], dict[str, Any]]) -> str:
    head = ["driver", "condition"] + [c[1] for c in COLS]
    lines = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    for (driver, cond), s in sorted(groups.items()):
        lines.append("| " + " | ".join([driver, cond] + [c[2](s) for c in COLS]) + " |")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("paths", nargs="+", help="run dirs or results.jsonl files; PATH=LABEL names the driver")
    p.add_argument("--md", type=Path)
    p.add_argument("--json", type=Path)
    a = p.parse_args(argv)
    rows: list[dict[str, Any]] = []
    for item in a.paths:
        path, _, label = item.partition("=")
        rows += load(Path(path), label or None)
    by: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for r in rows:
        by[(r["_driver"], str(r.get("condition") or "-"))].append(r)
        by[(r["_driver"], "all")].append(r)
    groups = {k: summarize(v) for k, v in by.items()}
    out = table(groups)
    print(out)
    if a.md:
        a.md.write_text(out + "\n", "utf-8")
    if a.json:
        a.json.write_text(json.dumps({f"{k[0]}|{k[1]}": v for k, v in groups.items()}, indent=2) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
