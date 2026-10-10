#!/usr/bin/env python3
"""Bench v2 report (Amendment 14, A14.7): per arm, overall and per category.

    tools/analyze_v2.py RUN_DIR [RUN_DIR ...] [--include-smoke] [--json OUT.json] [--md OUT.md]

Reads `results.jsonl` of each run (final rows of bench_version 2 only; infrastructure exclusions
dropped). For every arm, overall and per category:

* success: passed / trials with a Wilson 95% interval (a GUI-only violation is a fail, A14.2);
* mean turns, equivalent cost and wall time per trial;
* background operation (A14.4): background-clean trials, trials with a focus steal and the number
  of steals, trials where the real pointer moved, windows raised, input that leaked into the
  witness; and the headline form "34/50 passed, 0 focus steals";
* GUI-only (A14.2): violations, with each trial's evidence listed;
* interruptions (A14.5, IR-* only): shown, exercised, handled and completed rates.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import bench_v2 as v2  # noqa: E402
from analyze import wilson_interval  # noqa: E402

# Public names (bench naming of 10 Oct); every other arm is an internal baseline.
ARM_LABELS = {
    "cc-cua-driver-script": "Cua Driver",
    "cc-codex-cu": "Codex Computer-Use (Sky)",
    "cc-codex-cu-1007-browser": "Codex Computer-Use (Sky) 26.1007",
    "cc-claude-cu-helper": "Claude Computer-Use (Desktop helper)",
}


def load_rows(runs: list[Path], include_smoke: bool = False) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for run in runs:
        path = run / "results.jsonl" if run.is_dir() else run
        for line in path.read_text("utf-8").splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            if not row.get("final") or row.get("excluded") or row.get("bench_version") != v2.BENCH_VERSION:
                continue
            if row.get("smoke") and not include_smoke:
                continue
            rows.append(row)
    return rows


def _mean(values: list[float]) -> float | None:
    values = [v for v in values if isinstance(v, (int, float))]
    return round(sum(values) / len(values), 3) if values else None


def _rate(k: int, n: int) -> dict[str, Any]:
    ci = wilson_interval(k, n)
    return {"k": k, "n": n, "rate": round(k / n, 3) if n else None, "ci95": [round(x, 3) for x in ci] if ci else None}


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    n = len(rows)
    passed = sum(1 for r in rows if r.get("passed"))
    bg = [r.get("background") or {} for r in rows]
    measured = [b for b in bg if b.get("measured")]
    gui = [r.get("gui_only") or {} for r in rows]
    return {
        "trials": n,
        "success": _rate(passed, n),
        "passed_raw": sum(1 for r in rows if r.get("passed_raw")),
        "turns_mean": _mean([r.get("turns") for r in rows]),
        "cost_usd_mean": _mean([r.get("cost_usd") for r in rows]),
        "wall_s_mean": _mean([r.get("agent_wall_s") or r.get("wall_s") for r in rows]),
        "background": {
            "measured": len(measured),
            "clean": _rate(sum(1 for b in measured if b.get("clean")), len(measured)),
            "trials_with_focus_steal": sum(1 for b in measured if b.get("focus_steals")),
            "focus_steals": sum(int(b.get("focus_steals") or 0) for b in measured),
            "trials_pointer_moved": sum(1 for b in measured if b.get("pointer_moved")),
            "windows_raised": sum(int(b.get("windows_raised") or 0) for b in measured),
            "trials_window_raised": sum(1 for b in measured if b.get("windows_raised")),
            "windows_measured": sum(1 for b in measured if b.get("windows_measured")),
            "input_leaked": sum(int(b.get("input_leaked") or 0) for b in measured),
        },
        "gui_only_violations": sum(1 for g in gui if g.get("violation")),
    }


def interruptions(rows: list[dict[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    by_task: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for r in rows:
        if r.get("interruption"):
            by_task[r["task"]].append(r["interruption"])
    for task, items in sorted(by_task.items()):
        shown = [i for i in items if i.get("shown")]
        out[task] = {
            "kind": items[0].get("kind"),
            "trials": len(items),
            "shown": len(shown),
            "exercised": sum(1 for i in items if i.get("exercised")),
            "handled": _rate(sum(1 for i in shown if i.get("handled")), len(shown)),
            "completed": _rate(sum(1 for i in items if i.get("completed")), len(items)),
        }
    return out


def analyze(rows: list[dict[str, Any]]) -> dict[str, Any]:
    by_arm: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for r in rows:
        by_arm[r["arm"]].append(r)
    report: dict[str, Any] = {"bench_version": v2.BENCH_VERSION, "categories": v2.CATEGORIES, "arms": {}}
    for arm, arm_rows in sorted(by_arm.items()):
        cats = {}
        for cat in v2.CATEGORIES:
            sub = [r for r in arm_rows if r.get("category") == cat]
            cats[cat] = summarize(sub) if sub else None
        report["arms"][arm] = {
            "label": ARM_LABELS.get(arm, f"{arm} (internal baseline)"),
            "overall": summarize(arm_rows),
            "by_category": cats,
            "interruptions": interruptions(arm_rows),
            "violations": [
                {"trial_id": r["trial_id"], "evidence": (r.get("gui_only") or {}).get("violations")}
                for r in arm_rows
                if (r.get("gui_only") or {}).get("violation")
            ],
        }
    return report


def headline(s: dict[str, Any]) -> str:
    b = s["background"]
    return f"{s['success']['k']}/{s['trials']} passed, {b['focus_steals']} focus steals"


def _fmt(x: Any) -> str:
    return "n/a" if x is None else (f"{x:.2f}" if isinstance(x, float) else str(x))


def markdown(report: dict[str, Any]) -> str:
    lines = ["# Bench v2 results (Amendment 14)", ""]
    lines += ["## Overall", "", "| Setup | Headline | Success (95% CI) | Background-clean | Pointer moved | Windows raised | GUI-only violations | Turns | Cost | Wall s |", "|---|---|---|---|---|---|---|---|---|---|"]
    for arm, a in report["arms"].items():
        s = a["overall"]
        b = s["background"]
        ci = s["success"]["ci95"]
        lines.append(
            f"| {a['label']} | {headline(s)} | {_fmt(s['success']['rate'])} ({ci[0]:.2f}–{ci[1]:.2f}) | "
            f"{b['clean']['k']}/{b['measured']} | {b['trials_pointer_moved']} | {b['windows_raised']} | "
            f"{s['gui_only_violations']} | {_fmt(s['turns_mean'])} | {_fmt(s['cost_usd_mean'])} | {_fmt(s['wall_s_mean'])} |"
            if ci
            else f"| {a['label']} | no trials | | | | | | | | |"
        )
    for cat, label in report["categories"].items():
        lines += ["", f"## {label}", "", "| Setup | Passed | Background-clean | Focus steals | Turns | Cost | Wall s |", "|---|---|---|---|---|---|---|"]
        for arm, a in report["arms"].items():
            s = a["by_category"].get(cat)
            if not s:
                lines.append(f"| {a['label']} | no tasks | | | | | |")
                continue
            b = s["background"]
            lines.append(
                f"| {a['label']} | {s['success']['k']}/{s['trials']} | {b['clean']['k']}/{b['measured']} | "
                f"{b['focus_steals']} | {_fmt(s['turns_mean'])} | {_fmt(s['cost_usd_mean'])} | {_fmt(s['wall_s_mean'])} |"
            )
    lines += ["", "## Interruptions (IR probes)", "", "| Setup | Task | Kind | Shown | Exercised | Handled | Completed |", "|---|---|---|---|---|---|---|"]
    for arm, a in report["arms"].items():
        for task, i in a["interruptions"].items():
            lines.append(
                f"| {a['label']} | {task} | {i['kind']} | {i['shown']}/{i['trials']} | {i['exercised']}/{i['trials']} | "
                f"{i['handled']['k']}/{i['handled']['n']} | {i['completed']['k']}/{i['completed']['n']} |"
            )
    viol = [(a["label"], v) for a in report["arms"].values() for v in a["violations"]]
    lines += ["", "## GUI-only violations", ""]
    lines += [f"- {label}: {v['trial_id']}: {v['evidence']}" for label, v in viol] or ["None."]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("runs", nargs="+", type=Path)
    p.add_argument("--include-smoke", action="store_true")
    p.add_argument("--json", type=Path, default=None)
    p.add_argument("--md", type=Path, default=None)
    args = p.parse_args(argv)
    report = analyze(load_rows(args.runs, args.include_smoke))
    text = markdown(report)
    if args.json:
        args.json.write_text(json.dumps(report, indent=2) + "\n", "utf-8")
    if args.md:
        args.md.write_text(text, "utf-8")
    sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
