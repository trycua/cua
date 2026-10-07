#!/usr/bin/env python3
"""Summary of one mini-run (PREREGISTRATION.md, Amendment 6): per arm success, mean turns and cost against Codex CU,
the indicative match check of the overnight definition of done, and the most common failure classes with trial
paths. Descriptive only; the decision to run a full rerun is the A6.4 rule.

  analyze_mini.py RUN_DIR [--json FILE]          markdown on stdout (contains arm B numbers: keep it private)
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
import posthoc_calls as ph  # noqa: E402

B = "cc-codex-cu"
GUI = ["CDB-G02", "CDB-G03", "CDB-G04"]


def norm(text: str) -> str:
    """An error message with ids, numbers and quoted names removed, so similar failures group together."""
    t = re.sub(r"\s+", " ", text or "").strip()
    t = re.sub(r'"[^"]{0,80}"|\'[^\']{0,80}\'', "<s>", t)
    t = re.sub(r"\b[0-9a-f]{6,}\b|\d+(\.\d+)?", "N", t)
    return t[:110]


def rows_of(run: Path) -> list[dict]:
    rows = []
    for line in (run / "results.jsonl").read_text("utf-8").splitlines():
        if line.strip():
            r = json.loads(line)
            if r.get("final", True) and not r.get("excluded"):
                rows.append(r)
    return rows


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("run", type=Path)
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()
    rows = rows_of(args.run)
    arms = sorted({r["arm"] for r in rows}, key=lambda a: (a == B, a))
    by = defaultdict(list)
    for r in rows:
        by[r["arm"]].append(r)
    out: dict = {"run": args.run.name, "n": len(rows), "arms": {}}
    b = by.get(B, [])
    b_turns = st.mean(float(r.get("turns") or 0) for r in b) if b else None
    b_cost = st.mean(r.get("cost_usd") or 0 for r in b) if b else None
    p = print
    p(f"### Mini-run {args.run.name}: {len(rows)} trials\n")
    p("| Arm | Success | " + " | ".join(t.replace("CDB-", "") for t in sorted({r['task'] for r in rows})) + " | Turns | Turns / B | Cost USD | Cost / B |")
    tasks = sorted({r["task"] for r in rows})
    p("|---" * (len(tasks) + 6) + "|")
    for arm in arms:
        v = by[arm]
        k = sum(1 for r in v if r["passed"])
        turns = st.mean(float(r.get("turns") or 0) for r in v)
        cost = st.mean(r.get("cost_usd") or 0 for r in v)
        per = {t: (sum(1 for r in v if r["task"] == t and r["passed"]), sum(1 for r in v if r["task"] == t)) for t in tasks}
        out["arms"][arm] = {"passed": k, "n": len(v), "turns": turns, "cost_usd": cost, "per_task": per}
        rel = f"{turns / b_turns:.2f} | {cost:.3f} | {cost / b_cost:.2f}" if b else f"- | {cost:.3f} | -"
        p(f"| {arm} | {k}/{len(v)} | " + " | ".join(f"{a}/{n}" for a, n in per.values()) + f" | {turns:.1f} | {rel} |")
    if b:
        bk = out["arms"][B]
        p("\n**Indicative match check** (definition of done applied to this mini-run; the decision needs the full rerun):\n")
        for arm in arms:
            if arm == B:
                continue
            a = out["arms"][arm]
            overall = a["passed"] / a["n"] >= bk["passed"] / bk["n"]
            gui = all(a["per_task"].get(t, (0, 0))[0] * max(1, bk["per_task"].get(t, (0, 1))[1])
                      >= bk["per_task"].get(t, (0, 0))[0] * max(1, a["per_task"].get(t, (0, 1))[1]) for t in GUI if t in a["per_task"])
            turns_ok = a["turns"] <= 1.2 * bk["turns"]
            a["match"] = {"success_overall": overall, "each_gui_task": gui, "turns_within_1_2x": turns_ok}
            p(f"* {arm}: success >= B overall {'yes' if overall else 'no'}; >= B on each of G02-G04 {'yes' if gui else 'no'}; "
              f"turns <= 1.2x B {'yes' if turns_ok else 'no'} -> {'MATCH' if overall and gui and turns_ok else 'not yet'}")
    # failure classes of the Cua Driver arms
    classes: dict = defaultdict(list)
    for r in rows:
        if r["arm"] == B:
            continue
        trial = args.run / "trials" / r["trial_id"] / f"a{r['attempt']}"
        if not r["passed"]:
            if (r.get("turns") or 0) >= (r.get("max_turns") or 45):
                classes["turn cap reached (45)"].append(trial)
            failed_checks = [c for c, ok in (r.get("checks") or {}).items() if not ok]
            for c in failed_checks:
                classes[f"evaluator check failed: {c} ({r['task']})"].append(trial)
        stream = trial / "claude-stream.tsv"
        if stream.exists():
            for c in ph.calls(ph.events(stream)):
                if c["name"].startswith("mcp__") and c["error"]:
                    classes[f"{c['name'].split('__')[-1]} error: {norm(c['result'] or '')}"].append(trial)
    top = sorted(classes.items(), key=lambda kv: -len(kv[1]))
    p("\n**Top failure classes (Cua Driver arms; counts are occurrences, paths are trials):**\n")
    out["failure_classes"] = []
    for name, paths in top[:8]:
        uniq = sorted({str(x) for x in paths})
        out["failure_classes"].append({"class": name, "count": len(paths), "trials": uniq})
        p(f"* {len(paths)}x {name} — e.g. " + ", ".join(Path(u).parent.name for u in uniq[:3]))
    if args.json:
        args.json.write_text(json.dumps(out, indent=2, default=str) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
