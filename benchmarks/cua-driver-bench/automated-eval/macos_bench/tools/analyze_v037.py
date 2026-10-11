#!/usr/bin/env python3
"""Pre-registered analysis of run v037-full (PREREGISTRATION.md, Amendment 7).

  analyze_v037.py RUN_DIR [--out-json FILE] [--no-arm-b]       markdown on stdout

1. The decision rule (A7.4): pick the best Cua Driver arm (A or AX: more passed trials over the ten tasks; a tie goes
   to fewer mean turns), then check the three conditions of the overnight definition of done against arm B.
2. H1 to H3 of A5.4 for A against A0 on the GUI-heavy set (same tests and bootstrap).
3. Descriptive tables per task and arm.
Written before the first trial of the run. Arm B's numbers stay internal (CUA-1225); --no-arm-b drops arm B from the
tables but keeps the yes/no outcome of the decision rule.
"""

from __future__ import annotations

import argparse
import json
import statistics as st
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analyze_v035 as v5  # noqa: E402
import analyze_v036 as v6  # noqa: E402

A, AX, A0, B = "cc-cua-driver-main", "cc-cua-driver-script", "cc-cua-driver", "cc-codex-cu"
LABEL = {A: "A main", AX: "AX main + run_script", A0: "A0 0.34.0", B: "B Codex CU"}
GUI = ["CDB-G02", "CDB-G03", "CDB-G04"]


def turns(r: dict) -> float:
    return float(r.get("turns") or 0)


def decision(rows: list[dict]) -> dict:
    by = defaultdict(list)
    for r in rows:
        by[r["arm"]].append(r)
    def stats(arm: str) -> dict:
        v = by.get(arm, [])
        return {
            "n": len(v),
            "passed": sum(1 for r in v if r["passed"]),
            "turns": st.mean(turns(r) for r in v) if v else float("nan"),
            "per_gui": {t: (sum(1 for r in v if r["task"] == t and r["passed"]), sum(1 for r in v if r["task"] == t)) for t in GUI},
        }
    s = {arm: stats(arm) for arm in (A, AX, B)}
    cands = [arm for arm in (A, AX) if s[arm]["n"]]
    best = max(cands, key=lambda a: (s[a]["passed"] / s[a]["n"], -s[a]["turns"]))
    b, x = s[B], s[best]
    c1 = x["passed"] / x["n"] >= b["passed"] / b["n"]
    c2 = {t: x["per_gui"][t][0] / max(1, x["per_gui"][t][1]) >= b["per_gui"][t][0] / max(1, b["per_gui"][t][1]) for t in GUI}
    c3 = x["turns"] <= 1.2 * b["turns"]
    return {"best_arm": best, "stats": s, "success_ge_B": c1, "gui_ge_B": c2, "turns_le_1_2x_B": c3,
            "done": bool(c1 and all(c2.values()) and c3)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("run", type=Path)
    ap.add_argument("--out-json", type=Path)
    ap.add_argument("--no-arm-b", action="store_true")
    args = ap.parse_args()
    rows = v5.load(args.run / "results.jsonl")
    out: dict = {"rows": len(rows)}
    p = print
    d = decision(rows)
    out["decision"] = d
    p(f"# Run {args.run.name}: {len(rows)} counted trials\n")
    p("## Decision rule (A7.4)\n")
    p(f"Best Cua Driver arm: **{LABEL[d['best_arm']]}**.\n")
    p(f"* success >= B overall: {'yes' if d['success_ge_B'] else 'no'}")
    p("* success >= B on each GUI-only task: " + ", ".join(f"{t.replace('CDB-', '')} {'yes' if ok else 'no'}" for t, ok in d["gui_ge_B"].items()))
    p(f"* mean turns <= 1.2 x B: {'yes' if d['turns_le_1_2x_B'] else 'no'}")
    p(f"\n**Definition of done: {'MET' if d['done'] else 'NOT MET'}**\n")
    if not args.no_arm_b:
        p("| Arm | Passed | G02 | G03 | G04 | Mean turns |")
        p("|---|---|---|---|---|---|")
        for arm in (A, AX, B):
            x = d["stats"][arm]
            p(f"| {LABEL[arm]} | {x['passed']}/{x['n']} | " + " | ".join(f"{k}/{n}" for k, n in x["per_gui"].values()) + f" | {x['turns']:.1f} |")
        p("")
    by = defaultdict(list)
    for r in rows:
        by[(r["task"], r["arm"])].append(r)
    p("## H1 to H3 (A against A0, GUI-heavy set, A5.4 tests)\n")
    tp = v6.task_pairs_for(by, v6.PRIMARY, A, A0)
    if tp:
        h1, h2, h3 = v6.ratio_block(tp, v5.total_tokens), v6.ratio_block(tp, turns), v6.success_block(tp)
        out.update(H1=h1, H2=h2, H3=h3)
        p(f"* H1 tokens A/A0 {h1['geomean_ratio']:.2f} ({h1['ci95'][0]:.2f} to {h1['ci95'][1]:.2f}): {'supported' if h1['ci95'][1] < 1 else 'not supported'}")
        p(f"* H2 turns A/A0 {h2['geomean_ratio']:.2f} ({h2['ci95'][0]:.2f} to {h2['ci95'][1]:.2f}): {'supported' if h2['ci95'][1] < 1 else 'not supported'}")
        p(f"* H3 success A-A0 {h3['macro_diff']:+.2f} ({h3['ci95'][0]:+.2f} to {h3['ci95'][1]:+.2f}): {'supported' if h3['ci95'][0] >= v5.MARGIN else 'not supported'}")
    tp = v6.task_pairs_for(by, v6.ALL, AX, A)
    if tp:
        sx, tx = v6.success_block(tp), v6.ratio_block(tp, turns)
        out.update(AX_vs_A_success=sx, AX_vs_A_turns=tx)
        p(f"* AX against A (descriptive, ten tasks): success {sx['macro_diff']:+.2f} ({sx['ci95'][0]:+.2f} to {sx['ci95'][1]:+.2f}), turns {tx['geomean_ratio']:.2f} ({tx['ci95'][0]:.2f} to {tx['ci95'][1]:.2f})")
    arms = (A, AX, A0) if args.no_arm_b else (A, AX, A0, B)
    p("\n## Per task\n")
    p("| Task | Arm | n | Success | Score | Turns | Wall s | Tokens | Cost USD |")
    p("|---|---|---|---|---|---|---|---|---|")
    for t in v6.ALL:
        for arm in arms:
            v = by.get((t, arm), [])
            if v:
                p(f"| {t} | {LABEL[arm]} | {len(v)} | {sum(1 for r in v if r['passed'])}/{len(v)} | {st.mean(r['score'] or 0 for r in v):.2f} "
                  f"| {st.mean(turns(r) for r in v):.1f} | {st.mean(r['wall_s'] or 0 for r in v):.0f} | {st.mean(v5.total_tokens(r) for r in v):,.0f} | {st.mean(r['cost_usd'] or 0 for r in v):.3f} |")
    cost = {LABEL[a]: round(sum(r["cost_usd"] or 0 for r in rows if r["arm"] == a), 2) for a in arms}
    out["cost_usd"] = cost
    p("\nEquivalent cost: " + ", ".join(f"{k} ${v}" for k, v in cost.items()))
    if args.out_json:
        args.out_json.write_text(json.dumps(out, indent=2, default=str) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
