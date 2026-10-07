#!/usr/bin/env python3
"""Pre-registered analysis of run v036 (PREREGISTRATION.md, Amendment 5): A (main with the 0.36 fixes) vs A0 (0.34.0),
AS (A with SKILL.md in the system prompt) vs A, and B (Codex CU, descriptive only).

  analyze_v036.py RUN_DIR [--v035 V035_RUN_DIR] [--out-json FILE] [--no-arm-b]     markdown on stdout

Reads RUN_DIR/results.jsonl (final, non-smoke, non-excluded rows of complete task blocks). With --v035, also prints
the descriptive comparison of each arm with the same arm in run v035. --no-arm-b leaves arm B out of every table
(for anything that may be shared outside the team, CUA-1225). Written before the first trial of the run.
"""

from __future__ import annotations

import argparse
import json
import statistics as st
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analyze_v035 as v5  # noqa: E402  (load, bootstrap, wilson, token helpers, call breakdown)

A, A0, AS, B = "cc-cua-driver-main", "cc-cua-driver", "cc-cua-driver-main-skill", "cc-codex-cu"
LABEL = {A: "A main 0.36", A0: "A0 0.34.0", AS: "AS main 0.36 + skill in prompt", B: "B Codex CU"}
PRIMARY = v5.PRIMARY
ALL = v5.ALL
MARGIN = v5.MARGIN


def turns(r: dict) -> float:
    return float(r.get("turns") or 0)


def pairs(by: dict, task: str, x: str, y: str) -> list[tuple[dict, dict]]:
    a = {r["run_index"]: r for r in by.get((task, x), [])}
    b = {r["run_index"]: r for r in by.get((task, y), [])}
    return [(a[i], b[i]) for i in sorted(set(a) & set(b))]


def task_pairs_for(by: dict, tasks: list[str], x: str, y: str) -> dict:
    tp = {t: pairs(by, t, x, y) for t in tasks}
    return {t: p for t, p in tp.items() if p}


def ratio_block(tp: dict, metric) -> dict:
    point, lo, hi = v5.bootstrap(tp, lambda d, m=metric: v5.ratio_stat(d, m))
    per_task, lower, n = {}, 0, 0
    for t, prs in tp.items():
        mx = st.mean(metric(p[0]) for p in prs)
        my = st.mean(metric(p[1]) for p in prs)
        per_task[t] = {"x_mean": mx, "y_mean": my, "ratio": mx / my if my else None}
        lower += sum(1 for p in prs if metric(p[0]) < metric(p[1]))
        n += len(prs)
    return {"geomean_ratio": point, "ci95": [lo, hi], "share_of_pairs_lower": lower / n, "per_task": per_task}


def success_block(tp: dict) -> dict:
    point, lo, hi = v5.bootstrap(tp, v5.success_stat)
    per_task = {
        t: {"x": f"{sum(1 for p in prs if p[0]['passed'])}/{len(prs)}", "y": f"{sum(1 for p in prs if p[1]['passed'])}/{len(prs)}"}
        for t, prs in tp.items()
    }
    return {"macro_diff": point, "ci95": [lo, hi], "per_task": per_task}


def hypotheses(by: dict, tasks: list[str]) -> dict:
    out: dict = {}
    tp = task_pairs_for(by, tasks, A, A0)
    if tp:
        h1 = ratio_block(tp, v5.total_tokens)
        h2 = ratio_block(tp, turns)
        h3 = success_block(tp)
        h1["supported"] = h1["ci95"][1] < 1.0
        h2["supported"] = h2["ci95"][1] < 1.0
        h3["supported"] = h3["ci95"][0] >= MARGIN
        out.update(n_pairs_A_A0=sum(len(p) for p in tp.values()), H1_tokens_A_over_A0=h1, H2_turns_A_over_A0=h2,
                   H3_success_A_minus_A0=h3)
    tp = task_pairs_for(by, tasks, AS, A)
    if tp:
        h4 = success_block(tp)
        lo, hi = h4["ci95"]
        h4["reading"] = "AS better" if lo > 0 else "AS worse" if hi < 0 else "no resolvable difference"
        h5 = ratio_block(tp, turns)
        lo, hi = h5["ci95"]
        h5["reading"] = "AS fewer turns" if hi < 1 else "AS more turns" if lo > 1 else "no resolvable difference"
        tok = ratio_block(tp, v5.total_tokens)
        out.update(n_pairs_AS_A=sum(len(p) for p in tp.values()), H4_success_AS_minus_A=h4, H5_turns_AS_over_A=h5,
                   tokens_AS_over_A_reported=tok)
    return out


def arm_summary(rows: list[dict], arm: str, tasks: list[str]) -> dict:
    v = [r for r in rows if r["arm"] == arm and r["task"] in tasks]
    if not v:
        return {}
    by_task = defaultdict(list)
    for r in v:
        by_task[r["task"]].append(r)
    macro = st.mean(st.mean(float(bool(r["passed"])) for r in rs) for rs in by_task.values())
    return {
        "n": len(v),
        "passed": sum(1 for r in v if r["passed"]),
        "macro_success": macro,
        "turns": st.mean(turns(r) for r in v),
        "tokens": st.mean(v5.total_tokens(r) for r in v),
        "cost_usd": st.mean(r.get("cost_usd") or 0 for r in v),
        "wall_s": st.mean(r.get("wall_s") or 0 for r in v),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("run", type=Path)
    ap.add_argument("--v035", type=Path)
    ap.add_argument("--out-json", type=Path)
    ap.add_argument("--no-arm-b", action="store_true")
    args = ap.parse_args()
    arms = (A, A0, AS) if args.no_arm_b else (A, A0, AS, B)
    rows = [r for r in v5.load(args.run / "results.jsonl") if r["arm"] in arms]
    by: dict = defaultdict(list)
    for r in rows:
        by[(r["task"], r["arm"])].append(r)
    p = print
    out: dict = {"rows": len(rows)}
    p(f"# Run {args.run.name}: " + ", ".join(LABEL[a] for a in arms) + "\n")
    p(f"{len(rows)} counted trials (final, complete blocks).\n")
    p("## Pre-registered hypotheses (Amendment 5), GUI-heavy set\n")
    for label, tasks in (("GUI-heavy set (primary)", PRIMARY), ("all ten tasks (reported, not tested)", ALL)):
        h = hypotheses(by, tasks)
        out[label] = h
        if not h:
            p(f"{label}: no paired data\n")
            continue
        p(f"**{label}**\n")
        p("| Hypothesis | Estimate | 95% interval | Rule | Result |")
        p("|---|---|---|---|---|")
        if "H1_tokens_A_over_A0" in h:
            h1, h2, h3 = h["H1_tokens_A_over_A0"], h["H2_turns_A_over_A0"], h["H3_success_A_minus_A0"]
            p(f"| H1 tokens A/A0 | {h1['geomean_ratio']:.2f} | {h1['ci95'][0]:.2f} to {h1['ci95'][1]:.2f} | upper < 1.00 | {'supported' if h1['supported'] else 'not supported'} |")
            p(f"| H2 turns A/A0 | {h2['geomean_ratio']:.2f} | {h2['ci95'][0]:.2f} to {h2['ci95'][1]:.2f} | upper < 1.00 | {'supported' if h2['supported'] else 'not supported'} |")
            p(f"| H3 success A minus A0 | {h3['macro_diff']:+.2f} | {h3['ci95'][0]:+.2f} to {h3['ci95'][1]:+.2f} | lower >= {MARGIN:+.2f} | {'supported' if h3['supported'] else 'not supported'} |")
        if "H4_success_AS_minus_A" in h:
            h4, h5, tk = h["H4_success_AS_minus_A"], h["H5_turns_AS_over_A"], h["tokens_AS_over_A_reported"]
            p(f"| H4 success AS minus A | {h4['macro_diff']:+.2f} | {h4['ci95'][0]:+.2f} to {h4['ci95'][1]:+.2f} | two-sided: interval excludes 0 | {h4['reading']} |")
            p(f"| H5 turns AS/A | {h5['geomean_ratio']:.2f} | {h5['ci95'][0]:.2f} to {h5['ci95'][1]:.2f} | two-sided: interval excludes 1 | {h5['reading']} |")
            p(f"| tokens AS/A (reported) | {tk['geomean_ratio']:.2f} | {tk['ci95'][0]:.2f} to {tk['ci95'][1]:.2f} | none | - |")
        p("")
        p("| Task | Turns A | Turns A0 | Turns AS | Tokens A | Tokens A0 | Tokens AS | Pass A | Pass A0 | Pass AS |")
        p("|---|---|---|---|---|---|---|---|---|---|")
        for t in tasks:
            cell = {a: by.get((t, a), []) for a in (A, A0, AS)}
            if not any(cell.values()):
                continue
            def m(a, f):
                return f"{st.mean(f(r) for r in cell[a]):,.1f}" if cell[a] else "-"
            def k(a):
                return f"{sum(1 for r in cell[a] if r['passed'])}/{len(cell[a])}" if cell[a] else "-"
            p(f"| {t} | {m(A, turns)} | {m(A0, turns)} | {m(AS, turns)} | {m(A, v5.total_tokens)} | {m(A0, v5.total_tokens)} | {m(AS, v5.total_tokens)} | {k(A)} | {k(A0)} | {k(AS)} |")
        p("")
    for set_name, tasks in v5.SETS.items():
        p(f"## {set_name}\n")
        p("| Task | Arm | n | Success (Wilson 95%) | Score | Wall s | Turns | CU calls | Failed calls | Total tokens | Cost USD | Pointer moved | Focus stolen | Peeks | Flagged | Lab covered at start |")
        p("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
        for t in tasks:
            for arm in arms:
                v = by.get((t, arm), [])
                if not v:
                    continue
                n = len(v)
                k = sum(1 for r in v if r["passed"])
                lo, hi = v5.wilson(k, n)
                dist = [r.get("disturbance") or {} for r in v]
                covered = sum(1 for r in v if r.get("lab_unoccluded") is False or ("lab_occlusion" in r and r.get("lab_unoccluded") is None))
                p(
                    f"| {t} | {LABEL[arm]} | {n} | {k}/{n} ({lo:.2f}-{hi:.2f}) | {v5.ms([r['score'] or 0.0 for r in v], 2)} | {v5.ms([r['wall_s'] or 0 for r in v], 0)} "
                    f"| {v5.ms([turns(r) for r in v])} | {v5.ms([v5.cu_calls(r) for r in v])} | {sum((r.get('tool_calls') or {}).get('failed') or 0 for r in v)} "
                    f"| {v5.ms([v5.total_tokens(r) for r in v], 0)} | {v5.ms([r['cost_usd'] or 0 for r in v], 3)} "
                    f"| {sum(1 for d in dist if d.get('pointer_moved'))}/{n} | {sum(1 for d in dist if d.get('frontmost_changed'))}/{n} "
                    f"| {sum(1 for r in v if r.get('evaluator_peeks'))} | {sum(1 for r in v if r.get('side_door_flag'))} | {covered} |"
                )
        p("")
    br = v5.call_breakdown(args.run, rows)
    p("## Failed calls by tool\n")
    for arm in arms:
        p(f"* {LABEL[arm]}: " + (", ".join(f"{k} {v}" for k, v in br["failed"].get(arm, Counter()).most_common()) or "none"))
    p("\n## Use of the batching and lean-read options\n")
    for arm in (A, AS, A0):
        u = br["usage"].get(arm, Counter())
        p(f"* {LABEL[arm]}: " + ", ".join(f"{k} {v}" for k, v in u.items()))
    out["calls"] = {"failed": {a: dict(c) for a, c in br["failed"].items()}, "usage": {a: dict(c) for a, c in br["usage"].items()}}
    if args.v035:
        old = [r for r in v5.load(args.v035 / "results.jsonl") if r["arm"] in arms]
        p("\n## Compared with run v035 (descriptive, different runs: not paired, not tested)\n")
        p("| Set | Arm | v035 macro success | v036 macro success | v035 turns | v036 turns | v035 tokens | v036 tokens | v035 cost | v036 cost |")
        p("|---|---|---|---|---|---|---|---|---|---|")
        comp: dict = {}
        for set_name, tasks in (("GUI-heavy set", PRIMARY), ("all ten tasks", ALL)):
            for arm in arms:
                o, n = arm_summary(old, arm, tasks), arm_summary(rows, arm, tasks)
                comp[f"{set_name}/{arm}"] = {"v035": o, "v036": n}
                if not n:
                    continue
                f = lambda d, k, fmt: format(d[k], fmt) if d else "-"  # noqa: E731
                p(f"| {set_name} | {LABEL[arm]} | {f(o, 'macro_success', '.2f')} | {f(n, 'macro_success', '.2f')} | {f(o, 'turns', '.1f')} | {f(n, 'turns', '.1f')} "
                  f"| {f(o, 'tokens', ',.0f')} | {f(n, 'tokens', ',.0f')} | {f(o, 'cost_usd', '.3f')} | {f(n, 'cost_usd', '.3f')} |")
        p("\nThe v035 A arm is the main build at 365f5e3; v036's A arm is a later main build. AS has no v035 counterpart.")
        out["vs_v035"] = comp
    cost = {a: sum(r["cost_usd"] or 0 for r in rows if r["arm"] == a) for a in arms}
    p("\nEquivalent cost of counted trials: " + ", ".join(f"{LABEL[a]} ${c:.2f}" for a, c in cost.items()))
    out["cost_usd"] = cost
    if args.out_json:
        args.out_json.write_text(json.dumps(out, indent=2, default=str) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
