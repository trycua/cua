#!/usr/bin/env python3
"""Amendment 10 (A10.4): the Codex 26.1007 browser-surface arm set beside v038's arms and the arc arm.

  analyze_codex_rerun.py V038_DIR ARC_DIR NEW_DIR [--out-json FILE]      markdown on stdout

Separate runs on the same VM image, build, tasks and limits: every comparison is descriptive.
"""

from __future__ import annotations

import argparse
import json
import re
import statistics as st
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import analyze_v035 as v5  # noqa: E402
import analyze_v037 as v7  # noqa: E402
import posthoc_calls as ph  # noqa: E402

TASKS = v5.ALL
GUI = ["CDB-G02", "CDB-G03", "CDB-G04"]
NEW = "cc-codex-cu-1007-browser"
OLD = "cc-codex-cu"
LABEL = {
    "cc-cua-driver-script": "Cua Driver main + run_script (AX)",
    "cc-cua-driver-main": "Cua Driver main (A)",
    "cc-cua-driver": "Cua Driver 0.34.0 (A0)",
    OLD: "Codex CU 26.930, computer only (B)",
    NEW: "Codex CU 26.1007, browser+computer (B')",
    "cc-arc-driver": "arc-cua 0.1.1",
}


def cell(rows, arm, task=None):
    return [r for r in rows if r["arm"] == arm and (task is None or r["task"] == task)]


def summary(v):
    tok = sum(v5.total_tokens(r) for r in v)
    turns = sum(float(r.get("turns") or 0) for r in v)
    return {
        "n": len(v),
        "passed": sum(1 for r in v if r["passed"]),
        "turns": st.mean(float(r.get("turns") or 0) for r in v) if v else float("nan"),
        "cost": st.mean(r.get("cost_usd") or 0 for r in v) if v else float("nan"),
        "tokens_per_turn": tok / turns if turns else float("nan"),
        "wall": st.mean(r.get("wall_s") or 0 for r in v) if v else float("nan"),
    }


def browser_calls(run: Path, rows):
    """How often the new arm called the browser API, and how often that failed."""
    c = Counter()
    for r in rows:
        stream = run / "trials" / r["trial_id"] / f"a{r['attempt']}" / "claude-stream.tsv"
        if not stream.exists():
            continue
        for call in ph.calls(ph.events(stream)):
            if not call["name"].endswith("__js"):
                continue
            code = str((call["input"] or {}).get("code", ""))
            if re.search(r"listBrowsers|getBrowser|createBrowserTab|getTab|listTabs", code):
                c["browser_calls"] += 1
                if "Missing required Codex turn metadata" in (call["result"] or ""):
                    c["browser_metadata_errors"] += 1
            c["js_calls"] += 1
    return dict(c)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("v038", type=Path)
    ap.add_argument("arc", type=Path)
    ap.add_argument("new", type=Path)
    ap.add_argument("--out-json", type=Path)
    a = ap.parse_args()
    rows = v5.load(a.v038 / "results.jsonl") + [r for r in v5.load(a.arc / "results.jsonl") if r["arm"] == "cc-arc-driver"] + [
        r for r in v5.load(a.new / "results.jsonl") if r["arm"] == NEW
    ]
    arms = [x for x in LABEL if any(r["arm"] == x for r in rows)]
    p = print
    out: dict = {"arms": {}, "per_task": {}}
    p("## Standings (ten tasks, 5 runs per task and arm; separate runs, same VM image, build and limits)\n")
    p("| Arm | Passed | G02 | G03 | G04 | Mean turns | Tokens per turn | Cost per trial |")
    p("|---|---|---|---|---|---|---|---|")
    order = sorted(arms, key=lambda x: -summary(cell(rows, x))["passed"])
    for arm in order:
        s = summary(cell(rows, arm))
        g = {t: summary(cell(rows, arm, t))["passed"] for t in GUI}
        out["arms"][arm] = {**s, "gui": g}
        p(f"| {LABEL[arm]} | {s['passed']}/{s['n']} | {g['CDB-G02']}/5 | {g['CDB-G03']}/5 | {g['CDB-G04']}/5 | {s['turns']:.1f} | {s['tokens_per_turn']/1000:.1f}k | ${s['cost']:.2f} |")
    p("\n## Codex 26.1007 (browser+computer) against Codex 26.930 (computer only), per task\n")
    p("| Task | Passed 26.930 | Passed 26.1007 | Turns 26.930 | Turns 26.1007 | Cost 26.930 | Cost 26.1007 |")
    p("|---|---|---|---|---|---|---|")
    for t in TASKS:
        o, n = summary(cell(rows, OLD, t)), summary(cell(rows, NEW, t))
        out["per_task"][t] = {"old": o, "new": n}
        p(f"| {t} | {o['passed']}/{o['n']} | {n['passed']}/{n['n']} | {o['turns']:.1f} | {n['turns']:.1f} | ${o['cost']:.2f} | ${n['cost']:.2f} |")
    o, n = summary(cell(rows, OLD)), summary(cell(rows, NEW))
    p(f"| **All** | **{o['passed']}/{o['n']}** | **{n['passed']}/{n['n']}** | {o['turns']:.1f} | {n['turns']:.1f} | ${o['cost']:.2f} | ${n['cost']:.2f} |")
    bc = browser_calls(a.new, cell(rows, NEW))
    out["browser_calls"] = bc
    p(f"\nBrowser API use by the 26.1007 arm: {bc}")
    # the A7.4 decision rule with each Codex arm as B (cross-run, descriptive)
    for b in (OLD, NEW):
        sub = [dict(r, arm=("cc-codex-cu" if r["arm"] == b else r["arm"])) for r in rows if r["arm"] in ("cc-cua-driver-main", "cc-cua-driver-script", b)]
        d = v7.decision(sub)
        out[f"decision_vs_{b}"] = {k: d[k] for k in ("best_arm", "success_ge_B", "gui_ge_B", "turns_le_1_2x_B", "done")}
        p(f"\nA7.4 rule with B = {LABEL[b]}: best Cua arm {LABEL[d['best_arm']]}; success >= B {d['success_ge_B']}; "
          f"each GUI task {all(d['gui_ge_B'].values())} ({d['gui_ge_B']}); turns <= 1.2x B {d['turns_le_1_2x_B']} -> {'MET' if d['done'] else 'NOT MET'}")
    if a.out_json:
        a.out_json.write_text(json.dumps(out, indent=2, default=str) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
