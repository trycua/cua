#!/usr/bin/env python3
"""Speed and cost summary of one Stream K mini-run against a baseline run (PREREGISTRATION.md, Amendment 12).

Per Cua Driver arm, on the tasks the mini-run ran: passes, mean wall time per trial, model (API) time and tool
time (agent wall time minus API time), turns, tokens and equivalent cost per trial, beside the same arm and tasks
in the baseline run. It also splits each arm's context tokens by the tool whose result added them, weighted by the
turns that carried them (the context-growth method of A12.4). Descriptive only; the A12.4 guard decides nothing
about releases.

  analyze_speedcost.py RUN_DIR --baseline BASE_DIR [--json FILE]
"""

from __future__ import annotations

import argparse
import json
import statistics as st
from collections import Counter, defaultdict
from pathlib import Path

ARMS = {"cc-cua-driver-main": "A", "cc-cua-driver-script": "AX"}
GUARD_DROP = 0.15


def rows_of(run: Path) -> list[dict]:
    rows = []
    for line in (run / "results.jsonl").read_text("utf-8").splitlines():
        if line.strip():
            r = json.loads(line)
            if r.get("final", True) and not r.get("excluded"):
                rows.append(r)
    return rows


def tokens(r: dict) -> int:
    t = r.get("tokens") or {}
    return sum(int(t.get(k) or 0) for k in ("input", "output", "cache_read", "cache_write"))


def stats(rows: list[dict]) -> dict:
    if not rows:
        return {"n": 0}
    api = [(r.get("duration_api_ms") or 0) / 1000 for r in rows]
    agent = [float(r.get("agent_wall_s") or 0) for r in rows]
    return {
        "n": len(rows),
        "passed": sum(1 for r in rows if r.get("passed")),
        "rate": sum(1 for r in rows if r.get("passed")) / len(rows),
        "wall_s": st.mean(float(r.get("wall_s") or 0) for r in rows),
        "agent_wall_s": st.mean(agent),
        "api_s": st.mean(api),
        "tool_s": st.mean(a - b for a, b in zip(agent, api)),
        "turns": st.mean(float(r.get("turns") or 0) for r in rows),
        "tokens": st.mean(tokens(r) for r in rows),
        "cost": st.mean(float(r.get("cost_usd") or 0) for r in rows),
    }


def context_by_tool(run: Path, arm: str, tasks: set[str]) -> tuple[dict, int]:
    """Context growth after each turn, attributed to the tools called in that turn, times the turns after it."""
    weights: Counter = Counter()
    n = 0
    for stream in sorted((run / "trials").glob(f"*-{arm}/a*/claude-stream.tsv")):
        trial = stream.parent.parent.name
        if not any(trial.startswith(t + "-") for t in tasks):
            continue
        turns: list[dict] = []
        for line in stream.read_text("utf-8", errors="replace").splitlines():
            try:
                _, raw = line.split("\t", 1)
                m = json.loads(raw)
            except ValueError:
                continue
            if m.get("type") != "assistant":
                continue
            msg = m.get("message") or {}
            u = msg.get("usage") or {}
            ctx = sum(int(u.get(k) or 0) for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))
            if not turns or turns[-1]["id"] != msg.get("id"):
                turns.append({"id": msg.get("id"), "ctx": ctx, "tools": []})
            for c in msg.get("content") or []:
                if c.get("type") == "tool_use":
                    turns[-1]["tools"].append(str(c.get("name", "")).split("__")[-1])
        if len(turns) < 2:
            continue
        n += 1
        total = len(turns)
        weights["(first context x turns)"] += turns[0]["ctx"] * total
        for i, (a, b) in enumerate(zip(turns, turns[1:])):
            tools = sorted(set(a["tools"])) or ["(no tool)"]
            for tool in tools:
                weights[tool] += (b["ctx"] - a["ctx"]) * (total - i - 1) / len(tools)
    return {k: v / n for k, v in weights.items()} if n else {}, n


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("run", type=Path)
    ap.add_argument("--baseline", type=Path, required=True)
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()
    rows = [r for r in rows_of(args.run) if r["arm"] in ARMS]
    tasks = sorted({r["task"] for r in rows})
    base = [r for r in rows_of(args.baseline) if r["arm"] in ARMS and r["task"] in tasks]
    out: dict = {"run": args.run.name, "baseline": args.baseline.name, "tasks": tasks, "arms": {}}
    p = print
    p(f"### {args.run.name} against {args.baseline.name} on {', '.join(tasks)}\n")
    p("| Arm | Run | Passed | Wall s | Tool s | API s | Turns | Tokens | Cost |")
    p("|---|---|---|---|---|---|---|---|---|")
    for arm, label in ARMS.items():
        now = stats([r for r in rows if r["arm"] == arm])
        then = stats([r for r in base if r["arm"] == arm])
        if not now["n"]:
            continue
        for name, s in ((args.run.name, now), (args.baseline.name, then)):
            if s["n"]:
                p(f"| {label} | {name} | {s['passed']}/{s['n']} | {s['wall_s']:.1f} | {s['tool_s']:.1f} | {s['api_s']:.1f} "
                  f"| {s['turns']:.1f} | {s['tokens'] / 1000:.0f}k | ${s['cost']:.3f} |")
        guard = then["n"] and now["rate"] < then["rate"] - GUARD_DROP
        per_task = {}
        for t in tasks:
            a = [r for r in rows if r["arm"] == arm and r["task"] == t]
            b = [r for r in base if r["arm"] == arm and r["task"] == t]
            per_task[t] = {"now": f"{sum(1 for r in a if r.get('passed'))}/{len(a)}",
                           "baseline": f"{sum(1 for r in b if r.get('passed'))}/{len(b)}",
                           "wall_s_now": st.mean(float(r.get('wall_s') or 0) for r in a) if a else None,
                           "wall_s_baseline": st.mean(float(r.get('wall_s') or 0) for r in b) if b else None}
        ctx_now, n_now = context_by_tool(args.run, arm, set(tasks))
        ctx_then, n_then = context_by_tool(args.baseline, arm, set(tasks))
        out["arms"][label] = {"now": now, "baseline": then, "guard_flag": bool(guard), "per_task": per_task,
                              "context_by_tool_now": ctx_now, "context_by_tool_baseline": ctx_then}
    for label, a in out["arms"].items():
        p(f"\n**{label} per task (passes, mean wall s):** " + "; ".join(
            f"{t} {v['now']} vs {v['baseline']}, {v['wall_s_now'] or 0:.0f} vs {v['wall_s_baseline'] or 0:.0f} s"
            for t, v in a["per_task"].items()))
        if a["guard_flag"]:
            p(f"\n**{label}: A12.4 guard flagged** (success rate fell by more than {GUARD_DROP:.2f}).")
        keys = sorted(set(a["context_by_tool_now"]) | set(a["context_by_tool_baseline"]),
                      key=lambda k: -(a["context_by_tool_baseline"].get(k, 0) + a["context_by_tool_now"].get(k, 0)))[:10]
        p(f"\n{label} context tokens per trial by source (now vs baseline): " + "; ".join(
            f"{k} {a['context_by_tool_now'].get(k, 0) / 1000:.0f}k vs {a['context_by_tool_baseline'].get(k, 0) / 1000:.0f}k"
            for k in keys))
    if args.json:
        args.json.write_text(json.dumps(out, indent=1, default=float))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
