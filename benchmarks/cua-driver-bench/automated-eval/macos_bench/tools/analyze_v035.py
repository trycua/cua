#!/usr/bin/env python3
"""Pre-registered analysis of run v035 (PREREGISTRATION.md, Amendment 3): A (main build) vs A0 (0.34.0) vs B.

  analyze_v035.py RUN_DIR [--out-json FILE]      markdown on stdout

Reads RUN_DIR/results.jsonl (final, non-smoke, non-excluded rows of complete task blocks) and the trial streams
under RUN_DIR/trials for the call breakdowns. Written before the first trial of the run.
"""

from __future__ import annotations

import argparse
import json
import math
import random
import statistics as st
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import posthoc_calls as ph  # noqa: E402

A, A0, B = "cc-cua-driver-main", "cc-cua-driver", "cc-codex-cu"
ARMS = (A, A0, B)
LABEL = {A: "A main", A0: "A0 0.34.0", B: "B Codex CU"}
PRIMARY = ["CDB-S01", "CDB-S04", "CDB-G02", "CDB-G03", "CDB-G04", "MB-09", "MB-10", "MB-11"]
ALL = PRIMARY + ["CDB-S02", "CDB-S03"]
SETS = {
    "CDB suite with coding tools": ["CDB-S01", "CDB-S02", "CDB-S03", "CDB-S04"],
    "GUI-only CDB variant": ["CDB-G02", "CDB-G03", "CDB-G04"],
    "Probes": ["MB-09", "MB-10", "MB-11"],
}
BOOT = 10_000
SEED = 20261007
MARGIN = -0.15


def load(results: Path) -> list[dict]:
    """Final, counted rows of complete blocks. Completeness is recomputed from blocks.json: a row's
    `block_incomplete` flag is written when a run stops mid-block and is not cleared when a resumed run
    finishes that block."""
    rows = []
    for line in results.read_text("utf-8").splitlines():
        if not line.strip():
            continue
        d = json.loads(line)
        if d.get("smoke") or not d.get("final", True) or d.get("excluded"):
            continue
        rows.append(d)
    blocks_file = results.parent / "blocks.json"
    if blocks_file.is_file():
        blocks = json.loads(blocks_file.read_text("utf-8"))["blocks"]
        have: dict = defaultdict(set)
        for r in rows:
            have[r["block"]].add(r["trial_id"])
        complete = {bid for bid, b in blocks.items() if len(have.get(bid, set())) >= b["n_expected"]}
        rows = [r for r in rows if r["block"] in complete]
    else:
        rows = [r for r in rows if not r.get("block_incomplete")]
    return rows


def tok(row: dict, key: str) -> float:
    return float((row.get("tokens") or {}).get(key) or 0)


def total_tokens(row: dict) -> float:
    return sum(tok(row, k) for k in ("input", "output", "cache_read", "cache_write"))


def cu_calls(row: dict) -> int:
    return sum(v for k, v in (row.get("tool_calls") or {}).get("by_name", {}).items() if k.startswith("mcp__"))


def ms(values: list[float], digits: int = 1) -> str:
    if not values:
        return "n/a"
    sd = st.pstdev(values) if len(values) > 1 else 0.0
    return f"{st.mean(values):,.{digits}f} ({sd:,.{digits}f})"


def wilson(k: int, n: int, z: float = 1.96) -> tuple[float, float]:
    if n == 0:
        return (float("nan"), float("nan"))
    p = k / n
    den = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / den
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / den
    return (max(0.0, centre - half), min(1.0, centre + half))


def pairs(by: dict, task: str) -> list[tuple[dict, dict]]:
    a = {r["run_index"]: r for r in by.get((task, A), [])}
    a0 = {r["run_index"]: r for r in by.get((task, A0), [])}
    return [(a[i], a0[i]) for i in sorted(set(a) & set(a0))]


def geomean(xs: list[float]) -> float:
    xs = [x for x in xs if x > 0]
    return math.exp(sum(math.log(x) for x in xs) / len(xs)) if xs else float("nan")


def ratio_stat(task_pairs: dict, metric) -> float:
    ratios = []
    for prs in task_pairs.values():
        ma = st.mean(metric(p[0]) for p in prs)
        m0 = st.mean(metric(p[1]) for p in prs)
        if m0 > 0 and ma > 0:
            ratios.append(ma / m0)
    return geomean(ratios)


def success_stat(task_pairs: dict) -> float:
    return st.mean(
        st.mean(float(bool(p[0]["passed"])) for p in prs) - st.mean(float(bool(p[1]["passed"])) for p in prs)
        for prs in task_pairs.values()
    )


def bootstrap(task_pairs: dict, stat) -> tuple[float, float, float]:
    rng = random.Random(SEED)
    point = stat(task_pairs)
    draws = []
    for _ in range(BOOT):
        sample = {t: [rng.choice(prs) for _ in prs] for t, prs in task_pairs.items()}
        v = stat(sample)
        if not math.isnan(v):
            draws.append(v)
    draws.sort()
    lo = draws[int(0.025 * len(draws))]
    hi = draws[min(len(draws) - 1, int(0.975 * len(draws)))]
    return point, lo, hi


def hypotheses(by: dict, tasks: list[str]) -> dict:
    task_pairs = {t: pairs(by, t) for t in tasks}
    task_pairs = {t: p for t, p in task_pairs.items() if p}
    out: dict = {"tasks": sorted(task_pairs), "n_pairs": sum(len(p) for p in task_pairs.values())}
    if not task_pairs:
        return out
    for name, metric in (("H1_total_tokens", total_tokens), ("H2_turns", lambda r: float(r.get("turns") or 0))):
        point, lo, hi = bootstrap(task_pairs, lambda tp, m=metric: ratio_stat(tp, m))
        per_task = {}
        lower = 0
        n = 0
        for t, prs in task_pairs.items():
            ma = st.mean(metric(p[0]) for p in prs)
            m0 = st.mean(metric(p[1]) for p in prs)
            per_task[t] = {"A_mean": ma, "A0_mean": m0, "ratio": ma / m0 if m0 else None}
            lower += sum(1 for p in prs if metric(p[0]) < metric(p[1]))
            n += len(prs)
        out[name] = {
            "geomean_ratio_A_over_A0": point,
            "ci95": [lo, hi],
            "supported": hi < 1.0,
            "share_of_pairs_A_lower": lower / n,
            "per_task": per_task,
        }
    point, lo, hi = bootstrap(task_pairs, success_stat)
    worse = []
    for t, prs in task_pairs.items():
        ka = sum(1 for p in prs if p[0]["passed"])
        k0 = sum(1 for p in prs if p[1]["passed"])
        if ka <= k0 - 2:
            worse.append({"task": t, "A": f"{ka}/{len(prs)}", "A0": f"{k0}/{len(prs)}"})
    out["H3_success"] = {
        "macro_diff_A_minus_A0": point,
        "ci95": [lo, hi],
        "margin": MARGIN,
        "supported": lo >= MARGIN,
        "tasks_A_two_or_more_fewer_passes": worse,
    }
    return out


def counted_streams(run: Path, rows: list[dict]):
    """The stream of the counted (final) attempt of every counted trial."""
    for r in rows:
        stream = run / "trials" / r["trial_id"] / f"a{r['attempt']}" / "claude-stream.tsv"
        yield r, stream


def call_breakdown(run: Path, rows: list[dict]) -> dict:
    """Per arm and task: actions, observes, failed calls by tool, and A's use of run_actions / since / full_output."""
    stats: dict = defaultdict(lambda: defaultdict(list))
    failed: dict = defaultdict(Counter)
    usage: dict = defaultdict(Counter)
    for meta, stream in counted_streams(run, rows):
        if not stream.exists():
            continue
        arm, task = meta["arm"], meta["task"]
        cs = [c for c in ph.calls(ph.events(stream)) if c["name"].startswith("mcp__")]
        acts = obs = 0
        for c in cs:
            name = c["name"].split("__", 2)[-1]
            inp = c["input"] if isinstance(c["input"], dict) else {}
            if c["error"]:
                failed[arm][name] += 1
            if "codex-cu" in c["name"]:
                if name == "js":
                    a, o = ph.codex_actions(str(inp.get("code", "")))
                    acts += a
                    obs += o
                continue
            if name == "run_actions":
                steps = inp.get("steps") or []
                acts += len(steps) if isinstance(steps, list) else 1
                obs += 1 if inp.get("observe") else 0
                usage[arm]["run_actions calls"] += 1
                usage[arm]["run_actions steps"] += len(steps) if isinstance(steps, list) else 0
            elif name in ph.OBSERVE_CUA:
                obs += 1
            else:
                acts += 1
            if name == "get_window_state":
                usage[arm]["get_window_state calls"] += 1
                if inp.get("since"):
                    usage[arm]["get_window_state with since"] += 1
                if inp.get("full_output"):
                    usage[arm]["get_window_state with full_output"] += 1
        usage[arm]["trials"] += 1
        stats[arm][task].append((len(cs), acts, obs, sum(1 for c in cs if c["error"])))
    return {"stats": stats, "failed": failed, "usage": usage}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("run", type=Path)
    ap.add_argument("--out-json", type=Path)
    args = ap.parse_args()
    rows = load(args.run / "results.jsonl")
    by: dict = defaultdict(list)
    for r in rows:
        by[(r["task"], r["arm"])].append(r)
    out: dict = {"rows": len(rows)}
    p = print
    p(f"# Run {args.run.name}: A (main) vs A0 (0.34.0) vs B (Codex CU)\n")
    p(f"{len(rows)} counted trials (final, complete blocks).\n")
    p("## Pre-registered hypotheses (Amendment 3), GUI-heavy set\n")
    no_mb10 = [t for t in PRIMARY if t != "MB-10"]
    for label, tasks in (
        ("GUI-heavy set (primary)", PRIMARY),
        ("GUI-heavy set without MB-10 (post-hoc, A3.9 pointer carry-over)", no_mb10),
        ("all ten tasks (reported, not tested)", ALL),
    ):
        h = hypotheses(by, tasks)
        out[label] = h
        if "H1_total_tokens" not in h:
            p(f"{label}: no paired data\n")
            continue
        h1, h2, h3 = h["H1_total_tokens"], h["H2_turns"], h["H3_success"]
        p(f"**{label}**: {len(h['tasks'])} tasks, {h['n_pairs']} A/A0 pairs.\n")
        p("| Hypothesis | Estimate | 95% interval | Rule | Result |")
        p("|---|---|---|---|---|")
        p(f"| H1 tokens A/A0 (geometric mean of task ratios) | {h1['geomean_ratio_A_over_A0']:.2f} | {h1['ci95'][0]:.2f} to {h1['ci95'][1]:.2f} | upper < 1.00 | {'supported' if h1['supported'] else 'not supported'} |")
        p(f"| H2 turns A/A0 | {h2['geomean_ratio_A_over_A0']:.2f} | {h2['ci95'][0]:.2f} to {h2['ci95'][1]:.2f} | upper < 1.00 | {'supported' if h2['supported'] else 'not supported'} |")
        p(f"| H3 success A minus A0 (task-macro) | {h3['macro_diff_A_minus_A0']:+.2f} | {h3['ci95'][0]:+.2f} to {h3['ci95'][1]:+.2f} | lower >= {MARGIN:+.2f} | {'supported' if h3['supported'] else 'not supported'} |")
        p("")
        p(f"Pairs where A used fewer tokens: {h1['share_of_pairs_A_lower']:.0%}; fewer turns: {h2['share_of_pairs_A_lower']:.0%}. Tasks where A passed two or more fewer: {h3['tasks_A_two_or_more_fewer_passes'] or 'none'}.\n")
        p("| Task | Tokens A | Tokens A0 | Ratio | Turns A | Turns A0 | Ratio |")
        p("|---|---|---|---|---|---|---|")
        for t in h["tasks"]:
            a, b = h1["per_task"][t], h2["per_task"][t]
            p(f"| {t} | {a['A_mean']:,.0f} | {a['A0_mean']:,.0f} | {a['ratio']:.2f} | {b['A_mean']:.1f} | {b['A0_mean']:.1f} | {b['ratio']:.2f} |")
        p("")
    for set_name, tasks in SETS.items():
        p(f"## {set_name}\n")
        p("| Task | Arm | n | Success (Wilson 95%) | Score | Wall s | Turns | num_turns | CU calls | Failed calls | Input | Output | Cache read | Cache write | Total tokens | Cost USD | Pointer moved | Focus stolen | Peeks | Flagged |")
        p("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
        for t in tasks:
            for arm in ARMS:
                v = by.get((t, arm), [])
                if not v:
                    continue
                n = len(v)
                k = sum(1 for r in v if r["passed"])
                lo, hi = wilson(k, n)
                dist = [r.get("disturbance") or {} for r in v]
                p(
                    f"| {t} | {LABEL[arm]} | {n} | {k}/{n} ({lo:.2f}-{hi:.2f}) | {ms([r['score'] or 0.0 for r in v], 2)} | {ms([r['wall_s'] or 0 for r in v], 0)} "
                    f"| {ms([r['turns'] or 0 for r in v])} | {ms([r.get('num_turns_result') or 0 for r in v])} | {ms([cu_calls(r) for r in v])} "
                    f"| {sum((r.get('tool_calls') or {}).get('failed') or 0 for r in v)} | {ms([tok(r, 'input') for r in v], 0)} | {ms([tok(r, 'output') for r in v], 0)} "
                    f"| {ms([tok(r, 'cache_read') for r in v], 0)} | {ms([tok(r, 'cache_write') for r in v], 0)} | {ms([total_tokens(r) for r in v], 0)} "
                    f"| {ms([r['cost_usd'] or 0 for r in v], 3)} | {sum(1 for d in dist if d.get('pointer_moved'))}/{n} | {sum(1 for d in dist if d.get('frontmost_changed'))}/{n} "
                    f"| {sum(1 for r in v if r.get('evaluator_peeks'))} | {sum(1 for r in v if r.get('side_door_flag'))} |"
                )
        p("")
    p("## Probes by pointer reset (A3.9): phase 1 had no pointer reset, phase 2 parks the pointer\n")
    p("| Task | Arm | Pointer parked | n | Success | Pointer moved by agent |")
    p("|---|---|---|---|---|---|")
    for t in ("MB-09", "MB-10", "MB-11"):
        for arm in ARMS:
            for parked in (False, True):
                v = [r for r in by.get((t, arm), []) if bool(r.get("pointer_parked")) == parked]
                if v:
                    k = sum(1 for r in v if r["passed"])
                    moved = sum(1 for r in v if (r.get("disturbance") or {}).get("pointer_moved"))
                    p(f"| {t} | {LABEL[arm]} | {'yes' if parked else 'no'} | {len(v)} | {k}/{len(v)} | {moved}/{len(v)} |")
    p("")
    br = call_breakdown(args.run, rows)
    p("## Calls: actions and observes per trial (a run_actions step counts as an action)\n")
    p("| Arm | Task | Trials | MCP calls | Actions | Observes | Observes per action | Failed calls |")
    p("|---|---|---|---|---|---|---|---|")
    for arm in ARMS:
        for t in ALL:
            v = br["stats"].get(arm, {}).get(t, [])
            if not v:
                continue
            n = len(v)
            mc, ma, mo, mf = (sum(x[i] for x in v) / n for i in range(4))
            p(f"| {LABEL[arm]} | {t} | {n} | {mc:.1f} | {ma:.1f} | {mo:.1f} | {(mo / ma if ma else float('nan')):.2f} | {mf:.1f} |")
    p("\n## Failed calls by tool\n")
    for arm in ARMS:
        p(f"* {LABEL[arm]}: " + (", ".join(f"{k} {v}" for k, v in br["failed"].get(arm, Counter()).most_common()) or "none"))
    p("\n## Use of the new tools\n")
    for arm in (A, A0):
        u = br["usage"].get(arm, Counter())
        p(f"* {LABEL[arm]}: " + ", ".join(f"{k} {v}" for k, v in u.items()))
    cost = {arm: sum(r["cost_usd"] or 0 for r in rows if r["arm"] == arm) for arm in ARMS}
    p("\nEquivalent cost of counted trials: " + ", ".join(f"{LABEL[a]} ${c:.2f}" for a, c in cost.items()))
    out["calls"] = {
        "failed": {a: dict(c) for a, c in br["failed"].items()},
        "usage": {a: dict(c) for a, c in br["usage"].items()},
    }
    out["cost_usd"] = cost
    if args.out_json:
        args.out_json.write_text(json.dumps(out, indent=2, default=str) + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
