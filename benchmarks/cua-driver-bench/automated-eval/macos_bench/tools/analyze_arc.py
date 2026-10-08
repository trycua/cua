#!/usr/bin/env python3
"""Pre-registered analysis of the arc-driver arm (PREREGISTRATION.md, Amendment 9, CUA-1241).

  analyze_arc.py ARC_RUN_DIR REF_RUN_DIR [--out-json FILE] [--no-arm-b]       markdown on stdout

ARC_RUN_DIR holds the cc-arc-driver trials (the follow-on run); REF_RUN_DIR is the run they follow (v038), whose arms
ran the same tasks with the same seeds on the same VM image. Rows are the final, counted rows of complete blocks
(analyze_v035.load). Everything here is descriptive (A9.6):

1. Per arm over the tasks both runs share: passed trials, mean turns, tokens, wall time and cost.
2. arc against A (cc-cua-driver-main), paired by task and run index (same seed): task-macro success difference and
   the geometric-mean ratios of turns and tokens, with the A5.4 task-stratified bootstrap.
3. Per task and arm.
4. arc-driver's own signals from the trial streams: calls per tool, failed calls, `relaunch_for_accessibility`
   hints, `changed`/`stale` refusals, and how often the pixel tools of upstream issue #4 were used.
5. The most common failure classes of the arc arm, with trial paths.

Arm B (Codex CU) stays internal (CUA-1225): --no-arm-b leaves it out of every table. arc's own numbers may be shared.
"""

from __future__ import annotations

import argparse
import json
import re
import statistics as st
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analyze_v035 as v5  # noqa: E402
import analyze_v036 as v6  # noqa: E402
import posthoc_calls as ph  # noqa: E402

ARC, A, AX, A0, B = "cc-arc-driver", "cc-cua-driver-main", "cc-cua-driver-script", "cc-cua-driver", "cc-codex-cu"
LABEL = {ARC: "arc arc-driver 0.1.1", A: "A Cua main", AX: "AX Cua main + run_script", A0: "A0 Cua 0.34.0", B: "B Codex CU"}
PIXEL_TOOLS = ("click_at", "drag", "scroll_at")  # upstream issue #4: these land offset by the window origin
SIGNALS = {
    "relaunch_for_accessibility": re.compile(r"relaunch_for_accessibility"),
    "changed (snapshot structure moved)": re.compile(r'"status"\s*:\s*"changed"'),
    "stale (element changed)": re.compile(r'"(?:status|code)"\s*:\s*"stale"'),
}


def turns(r: dict) -> float:
    return float(r.get("turns") or 0)


def norm(text: str) -> str:
    t = re.sub(r"\s+", " ", text or "").strip()
    t = re.sub(r'"[^"]{0,80}"|\'[^\']{0,80}\'', "<s>", t)
    t = re.sub(r"\b[0-9a-f]{6,}\b|\d+(\.\d+)?", "N", t)
    return t[:110]


def summary(rows: list[dict]) -> dict:
    return {
        "n": len(rows),
        "passed": sum(1 for r in rows if r["passed"]),
        "turns": st.mean(turns(r) for r in rows) if rows else float("nan"),
        "tokens": st.mean(v5.total_tokens(r) for r in rows) if rows else float("nan"),
        "wall_s": st.mean(r.get("wall_s") or 0 for r in rows) if rows else float("nan"),
        "cost_usd": st.mean(r.get("cost_usd") or 0 for r in rows) if rows else float("nan"),
    }


def stream_signals(run: Path, rows: list[dict]) -> dict:
    calls: Counter = Counter()
    failed: Counter = Counter()
    signals: Counter = Counter()
    classes: dict = defaultdict(list)
    pixel_trials = 0
    for r in rows:
        trial = run / "trials" / r["trial_id"] / f"a{r['attempt']}"
        stream = trial / "claude-stream.tsv"
        used_pixel = False
        if stream.exists():
            for c in ph.calls(ph.events(stream)):
                if not c["name"].startswith("mcp__"):
                    continue
                tool = c["name"].split("__")[-1]
                calls[tool] += 1
                used_pixel |= tool in PIXEL_TOOLS
                text = c.get("result") or ""
                for name, pat in SIGNALS.items():
                    if pat.search(text):
                        signals[name] += 1
                if c["error"]:
                    failed[tool] += 1
                    classes[f"{tool} error: {norm(text)}"].append(trial)
        pixel_trials += used_pixel
        if not r["passed"]:
            if turns(r) >= (r.get("max_turns") or 45):
                classes["turn cap reached"].append(trial)
            for check, ok in (r.get("checks") or {}).items():
                if not ok:
                    classes[f"evaluator check failed: {check} ({r['task']})"].append(trial)
    top = sorted(classes.items(), key=lambda kv: -len(kv[1]))[:10]
    return {
        "calls": dict(calls.most_common()),
        "failed": dict(failed.most_common()),
        "signals": dict(signals),
        "trials_using_pixel_tools": pixel_trials,
        "failure_classes": [{"class": k, "count": len(v), "trials": sorted({str(x) for x in v})} for k, v in top],
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("arc_run", type=Path)
    ap.add_argument("ref_run", type=Path)
    ap.add_argument("--out-json", type=Path)
    ap.add_argument("--no-arm-b", action="store_true")
    args = ap.parse_args()
    arc_rows = [r for r in v5.load(args.arc_run / "results.jsonl") if r["arm"] == ARC]
    ref_rows = [r for r in v5.load(args.ref_run / "results.jsonl") if r["arm"] != ARC]
    tasks = [t for t in v6.ALL if any(r["task"] == t for r in arc_rows)]
    ref_rows = [r for r in ref_rows if r["task"] in tasks]
    rows = arc_rows + ref_rows
    arms = [a for a in (ARC, A, AX, A0, B) if any(r["arm"] == a for r in rows) and not (args.no_arm_b and a == B)]
    by = defaultdict(list)
    for r in rows:
        by[(r["task"], r["arm"])].append(r)
    out: dict = {"arc_run": args.arc_run.name, "ref_run": args.ref_run.name, "tasks": tasks, "arms": {}}
    p = print
    p(f"# arc-driver follow-on: {args.arc_run.name} against {args.ref_run.name}\n")
    p(f"{len(arc_rows)} counted arc trials over {len(tasks)} tasks; reference arms from {args.ref_run.name} on the same tasks.\n")
    p("## 1. Per arm\n")
    p("| Arm | Passed | Mean turns | Tokens | Wall s | Cost USD |")
    p("|---|---|---|---|---|---|")
    for arm in arms:
        s = summary([r for r in rows if r["arm"] == arm])
        out["arms"][arm] = s
        p(f"| {LABEL[arm]} | {s['passed']}/{s['n']} | {s['turns']:.1f} | {s['tokens']:,.0f} | {s['wall_s']:.0f} | {s['cost_usd']:.3f} |")
    p("\n## 2. arc against A (paired by task and run index, task-stratified bootstrap)\n")
    tp = v6.task_pairs_for(by, tasks, ARC, A)
    if tp:
        sx, tx, kx = v6.success_block(tp), v6.ratio_block(tp, turns), v6.ratio_block(tp, v5.total_tokens)
        out.update(arc_vs_A_success=sx, arc_vs_A_turns=tx, arc_vs_A_tokens=kx)
        p(f"* success arc - A: {sx['macro_diff']:+.2f} ({sx['ci95'][0]:+.2f} to {sx['ci95'][1]:+.2f})")
        p(f"* turns arc / A: {tx['geomean_ratio']:.2f} ({tx['ci95'][0]:.2f} to {tx['ci95'][1]:.2f})")
        p(f"* tokens arc / A: {kx['geomean_ratio']:.2f} ({kx['ci95'][0]:.2f} to {kx['ci95'][1]:.2f})")
    else:
        p("No pairs (the reference run has no A rows on these tasks).")
    p("\n## 3. Per task\n")
    p("| Task | Arm | n | Success | Score | Turns | Wall s | Tokens | Cost USD |")
    p("|---|---|---|---|---|---|---|---|---|")
    for t in tasks:
        for arm in arms:
            v = by.get((t, arm), [])
            if v:
                p(f"| {t} | {LABEL[arm]} | {len(v)} | {sum(1 for r in v if r['passed'])}/{len(v)} | {st.mean(r['score'] or 0 for r in v):.2f} "
                  f"| {st.mean(turns(r) for r in v):.1f} | {st.mean(r['wall_s'] or 0 for r in v):.0f} | {st.mean(v5.total_tokens(r) for r in v):,.0f} | {st.mean(r['cost_usd'] or 0 for r in v):.3f} |")
    sig = stream_signals(args.arc_run, arc_rows)
    out["arc_streams"] = sig
    p("\n## 4. arc-driver signals from the streams\n")
    p("* calls per tool: " + ", ".join(f"{k} {v}" for k, v in sig["calls"].items()))
    p("* failed calls: " + (", ".join(f"{k} {v}" for k, v in sig["failed"].items()) or "none"))
    p("* " + ", ".join(f"{k}: {v}" for k, v in sig["signals"].items()) if sig["signals"] else "* no relaunch/changed/stale signals")
    p(f"* trials that used click_at, drag or scroll_at (upstream issue #4): {sig['trials_using_pixel_tools']}/{len(arc_rows)}")
    flagged = sum(1 for r in arc_rows if r.get("force_accessibility"))
    leftovers = sum(int(r.get("arc_leftover_killed") or 0) for r in arc_rows)
    p(f"* trials with Chrome/Electron started with --force-renderer-accessibility: {flagged}/{len(arc_rows)}; leftover arc servers killed after trials: {leftovers}")
    p("\n## 5. Failure classes (arc arm)\n")
    for c in sig["failure_classes"]:
        p(f"* {c['count']}x {c['class']} (e.g. " + ", ".join(Path(u).parent.name for u in c["trials"][:3]) + ")")
    if args.out_json:
        args.out_json.write_text(json.dumps(out, indent=2, default=str) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
