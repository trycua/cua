"""The run report: one markdown file per test and OS, beside its screenshots,
video and logs, plus one summary across the run."""

from __future__ import annotations

import json
from pathlib import Path


def _cell(s: str, n: int = 160) -> str:
    return str(s).replace("|", "\\|").replace("\n", " ")[:n]


def write(out: Path, r: dict) -> Path:
    lines = [f"# {r['test']} on {r['os']}: {r['verdict'].upper()}", ""]
    if r.get("title"):
        lines += [r["title"], ""]
    lines += [
        f"- Mode: {r['mode']}"
        + (f" (replay broke: {r['replay_broke']}; ran live)" if r.get("replay_broke") else ""),
        f"- Build: {r.get('build', '?')}",
        f"- Model: {r.get('model', '-')}",
        f"- Agent: {r.get('agent_exit', '-')}, {r.get('steps', 0)} tool calls, {r.get('turns', 0)} model turns, "
        f"{r.get('seconds', 0)} s, ${r.get('cost_usd', 0):.2f}",
    ]
    if r.get("issues"):
        lines.append("- Issues: " + ", ".join(r["issues"]))
    if r.get("error"):
        lines += ["", f"**Could not run:** {r['error']}"]
    lines += [
        "",
        "## Hard checks (these decide the verdict)",
        "",
        "| | Check | Evidence |",
        "|---|---|---|",
    ]
    for c in r.get("checks", []):
        mark = "skip" if c.get("skipped") else ("pass" if c["ok"] else "FAIL")
        lines.append(f"| {mark} | {_cell(c['name'], 60)} | {_cell(c['evidence'], 300)} |")
    if r.get("agent_text"):
        lines += ["", "## What the agent said", "", "```", r["agent_text"].strip()[-2000:], "```"]
    if r.get("review"):
        lines += ["", "## Model review (advisory; never sets the verdict)", "", r["review"].strip()]
    if r.get("dialogs_seen"):
        lines += ["", "## Window titles seen during the run", ""]
        lines += [
            f"- {at} s: {_cell(t, 120)}"
            for t, at in sorted(r["dialogs_seen"].items(), key=lambda x: x[1])
        ]
    shots = (
        sorted((out / "screenshots").glob("*.png"))[-6:] if (out / "screenshots").exists() else []
    )
    if shots:
        lines += ["", "## Last screenshots", ""]
        lines += [f"![{p.name}](screenshots/{p.name})" for p in shots]
    video = next(iter(sorted(out.rglob("recording.mp4"))), None)
    if video:
        lines += ["", f"Video: [{video.relative_to(out)}]({video.relative_to(out)})"]
    calls = out / "calls.jsonl"
    if calls.exists():
        lines += ["", "## Tool calls", "", "| # | Tool | Arguments | Result |", "|---|---|---|---|"]
        for line in calls.read_text(encoding="utf-8").splitlines():
            e = json.loads(line)
            if "step" in e:
                args = {k: v for k, v in (e.get("args") or {}).items() if k not in ("session",)}
                first = ((e.get("text") or "").strip().splitlines() or [""])[0]
                res = ("error: " if e.get("is_error") else "") + first
                lines.append(
                    f"| {e['step']} | {e['tool']} | {_cell(json.dumps(args), 120)} | {_cell(res, 120)} |"
                )
            elif "refused" in e:
                lines.append(f"| - | {e['refused']} | refused | {e['why']} |")
    if r.get("setup_log"):
        lines += ["", "## Setup log", "", "```", *r["setup_log"][-60:], "```"]
    path = out / "report.md"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    (out / "result.json").write_text(
        json.dumps({k: v for k, v in r.items() if k != "setup_log"}, indent=2), encoding="utf-8"
    )
    return path


def summary(results: list[dict]) -> str:
    lines = [
        "| Test | OS | Verdict | Mode | Tool calls | Cost | Failed checks |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in results:
        failed = [c["name"] for c in r.get("checks", []) if not c["ok"] and not c.get("skipped")]
        why = "; ".join(failed) or (r.get("error") or "")
        if r.get("replay_broke"):
            why = f"replay broke: {r['replay_broke']}" + (f"; {why}" if why else "")
        lines.append(
            f"| {r['test']} | {r['os']} | {r['verdict']} | {r['mode']} | {r.get('steps', 0)} | "
            f"${r.get('cost_usd', 0):.2f} | {_cell(why, 200)} |"
        )
    total = sum(r.get("cost_usd", 0) for r in results)
    lines += ["", f"Model cost: ${total:.2f} across {len(results)} run(s)."]
    return "\n".join(lines) + "\n"
