#!/usr/bin/env python3
"""Pre-registered analysis of the Claude computer-use arms (PREREGISTRATION.md, Amendment 11), written before their
first trial.

  analyze_claude_cu.py RUN_DIR REF_RUN_DIR [--extra RUN_DIR ...] [--arm ARM] [--out-json FILE]

RUN_DIR holds the follow-on arm's trials (cc-claude-cu-helper, or cc-claude-cu-builtin); REF_RUN_DIR is v038, whose
arms ran the same tasks with the same seeds on the same VM image. --extra adds other follow-on runs on the same tasks
(v038-arc, v038-codex1007) to the standings table. Rows are the final, counted rows of complete blocks
(analyze_v035.load). Everything here is descriptive; these are separate runs, not an interleaved comparison.

1. Standings: per arm over the tasks the follow-on run covers: passed trials, mean turns, tokens, wall time, cost.
2. The follow-on arm against A (cc-cua-driver-main), paired by task and run index: task-macro success difference and
   the geometric-mean turns and tokens ratios, with the A5.4 task-stratified bootstrap.
3. Per task and arm.
4. The arm's own signals from its streams: calls per tool, failed calls, and input calls the helper reported as not
   delivered.
5. The most common failure classes, with trial paths.
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

HELPER, BUILTIN = "cc-claude-cu-helper", "cc-claude-cu-builtin"
A, AX, A0, B, ARC, B1007 = (
    "cc-cua-driver-main", "cc-cua-driver-script", "cc-cua-driver", "cc-codex-cu", "cc-arc-driver",
    "cc-codex-cu-1007-browser",
)
LABEL = {
    HELPER: "Claude Desktop 2.31226.0 computer-use helper via a minimal adapter",
    BUILTIN: "Claude Code's built-in computer use",
    A: "A Cua Driver main",
    AX: "AX Cua Driver main + run_script",
    A0: "A0 Cua Driver 0.34.0",
    B: "B Codex CU 26.930",
    B1007: "B' Codex CU 26.1007 (browser+computer)",
    ARC: "arc-driver 0.1.1",
}
ORDER = (HELPER, BUILTIN, AX, A, A0, B, B1007, ARC)
NOT_DELIVERED = re.compile(r'"delivered"\s*:\s*false|did not deliver')


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
    undelivered = 0
    classes: dict = defaultdict(list)
    for r in rows:
        trial = run / "trials" / r["trial_id"] / f"a{r['attempt']}"
        stream = trial / "claude-stream.tsv"
        if stream.exists():
            for c in ph.calls(ph.events(stream)):
                tool = c["name"].split("__")[-1] if c["name"].startswith("mcp__") else c["name"]
                calls[tool] += 1
                text = c.get("result") or ""
                if NOT_DELIVERED.search(text):
                    undelivered += 1
                if c["error"]:
                    failed[tool] += 1
                    classes[f"{tool} error: {norm(text)}"].append(trial)
        if not r["passed"]:
            if turns(r) >= (r.get("max_turns") or 45):
                classes["turn cap reached"].append(trial)
            if r.get("status") == "timeout":
                classes["wall-time limit reached"].append(trial)
            for check, ok in (r.get("checks") or {}).items():
                if not ok:
                    classes[f"evaluator check failed: {check} ({r['task']})"].append(trial)
    top = sorted(classes.items(), key=lambda kv: -len(kv[1]))[:12]
    return {
        "calls": dict(calls.most_common()),
        "failed": dict(failed.most_common()),
        "inputs_not_delivered": undelivered,
        "failure_classes": [{"class": k, "count": len(v), "trials": sorted({str(x) for x in v})} for k, v in top],
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("run", type=Path)
    ap.add_argument("ref_run", type=Path)
    ap.add_argument("--extra", type=Path, action="append", default=[])
    ap.add_argument("--arm", default=HELPER)
    ap.add_argument("--out-json", type=Path)
    args = ap.parse_args()
    arm_rows = [r for r in v5.load(args.run / "results.jsonl") if r["arm"] == args.arm]
    tasks = [t for t in v6.ALL if any(r["task"] == t for r in arm_rows)]
    others = [r for r in v5.load(args.ref_run / "results.jsonl") if r["arm"] != args.arm]
    for extra in args.extra:
        others += [r for r in v5.load(extra / "results.jsonl") if r["arm"] in (ARC, B1007)]
    rows = arm_rows + [r for r in others if r["task"] in tasks]
    arms = [a for a in ORDER if any(r["arm"] == a for r in rows)]
    by = defaultdict(list)
    for r in rows:
        by[(r["task"], r["arm"])].append(r)
    out: dict = {"run": args.run.name, "ref_run": args.ref_run.name, "extra": [e.name for e in args.extra],
                 "arm": args.arm, "tasks": tasks, "arms": {}}
    p = print
    p(f"# {LABEL[args.arm]}: {args.run.name} against {args.ref_run.name}\n")
    p(f"{len(arm_rows)} counted trials of {args.arm} over {len(tasks)} tasks. Other arms: {args.ref_run.name}"
      + (", " + ", ".join(e.name for e in args.extra) if args.extra else "") + " on the same tasks (separate runs).\n")
    p("## 1. Standings\n")
    p("| Arm | Passed | Mean turns | Tokens | Wall s | Cost USD |")
    p("|---|---|---|---|---|---|")
    for arm in sorted(arms, key=lambda a: -summary([r for r in rows if r["arm"] == a])["passed"]):
        s = summary([r for r in rows if r["arm"] == arm])
        out["arms"][arm] = s
        p(f"| {LABEL[arm]} | {s['passed']}/{s['n']} | {s['turns']:.1f} | {s['tokens']:,.0f} | {s['wall_s']:.0f} | {s['cost_usd']:.3f} |")
    p(f"\n## 2. {args.arm} against A (paired by task and run index, task-stratified bootstrap)\n")
    tp = v6.task_pairs_for(by, tasks, args.arm, A)
    if tp:
        sx, tx, kx = v6.success_block(tp), v6.ratio_block(tp, turns), v6.ratio_block(tp, v5.total_tokens)
        out.update(vs_A_success=sx, vs_A_turns=tx, vs_A_tokens=kx)
        p(f"* success minus A: {sx['macro_diff']:+.2f} ({sx['ci95'][0]:+.2f} to {sx['ci95'][1]:+.2f})")
        p(f"* turns / A: {tx['geomean_ratio']:.2f} ({tx['ci95'][0]:.2f} to {tx['ci95'][1]:.2f})")
        p(f"* tokens / A: {kx['geomean_ratio']:.2f} ({kx['ci95'][0]:.2f} to {kx['ci95'][1]:.2f})")
    else:
        p("No pairs.")
    p("\n## 3. Per task\n")
    p("| Task | Arm | n | Success | Score | Turns | Wall s | Tokens | Cost USD |")
    p("|---|---|---|---|---|---|---|---|---|")
    for t in tasks:
        for arm in arms:
            v = by.get((t, arm), [])
            if v:
                p(f"| {t} | {LABEL[arm]} | {len(v)} | {sum(1 for r in v if r['passed'])}/{len(v)} | {st.mean(r['score'] or 0 for r in v):.2f} "
                  f"| {st.mean(turns(r) for r in v):.1f} | {st.mean(r['wall_s'] or 0 for r in v):.0f} | {st.mean(v5.total_tokens(r) for r in v):,.0f} | {st.mean(r['cost_usd'] or 0 for r in v):.3f} |")
    sig = stream_signals(args.run, arm_rows)
    out["streams"] = sig
    p(f"\n## 4. {args.arm} signals from the streams\n")
    p("* calls per tool: " + ", ".join(f"{k} {v}" for k, v in sig["calls"].items()))
    p("* failed calls: " + (", ".join(f"{k} {v}" for k, v in sig["failed"].items()) or "none"))
    p(f"* input calls the helper reported as not delivered: {sig['inputs_not_delivered']}")
    leftovers = sum(int(r.get("cu_helper_leftover_killed") or 0) for r in arm_rows)
    p(f"* leftover adapter/helper processes killed after trials: {leftovers}")
    p("\n## 5. Failure classes\n")
    for c in sig["failure_classes"]:
        p(f"* {c['count']}x {c['class']} (e.g. " + ", ".join(Path(u).parent.name for u in c["trials"][:3]) + ")")
    if args.out_json:
        args.out_json.write_text(json.dumps(out, indent=2, default=str) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
