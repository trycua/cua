#!/usr/bin/env python3
"""Analysis of a run_bench.py results file with the pre-registered statistics in analyze.py.

    analyze_bench.py runs/<run>/results.jsonl --out-md report.md --out-json report.json

What this adapter does, and nothing else:

* keeps only final rows of complete task blocks (drops smoke rows, retried attempts and rows flagged
  ``block_incomplete``); ``--include-smoke`` and ``--include-incomplete`` exist for plumbing checks only;
* maps the Claude arms onto analyze.py's A/B slots (A = cc-cua-driver, B = cc-codex-cu) and the extra
  ``max_turns`` status onto ``agent_error`` (the original is kept in ``status_detail``);
* appends a Claude Code section: equivalent cost, tokens, turns, ToolSearch turns, baseline prompt size,
  quota and pause log, which analyze.py does not know about.

Failures are never dropped; excluded (infrastructure) trials are counted by analyze.py as before.
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import tempfile
from collections import defaultdict
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import analyze  # noqa: E402

ARM_A, ARM_B = "cc-cua-driver", "cc-codex-cu"


def select_rows(
    rows: list[dict[str, Any]],
    include_smoke: bool = False,
    include_incomplete: bool = False,
    phase1_only: bool = False,
) -> list[dict[str, Any]]:
    out = []
    for row in rows:
        if row.get("smoke") and not include_smoke:
            continue
        if not row.get("final", True):
            continue
        if row.get("block_incomplete") and not include_incomplete:
            continue
        if phase1_only and row.get("phase") != 1:
            continue
        out.append(row)
    return out


def to_pilot_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    mapped = []
    for row in rows:
        row = dict(row)
        row["schema"] = analyze.SCHEMA
        row["status_detail"] = row["status"]
        if row["status"] == "max_turns":
            row["status"] = "agent_error"
        mapped.append(row)
    return mapped


def _stat(values: list[float]) -> str:
    values = [v for v in values if v is not None]
    if not values:
        return "n/a"
    return (
        f"{statistics.mean(values):.3g} (median {statistics.median(values):.3g}, n={len(values)})"
    )


def claude_section(rows: list[dict[str, Any]], run_dir: Path | None) -> str:
    lines = ["", "## Claude Code run details", ""]
    lines.append(
        "Equivalent cost is `total_cost_usd` from each trial's result event (subscription run, nothing billed per token). "
        "`input` excludes cache tokens; `cache_read` and `cache_write` are separate."
    )
    lines.append("")
    lines += [
        "| arm | trials | passed | equivalent cost sum USD | cost per trial | input | output | cache read | cache write | turns | ToolSearch turns | baseline prompt tokens | declared DONE |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for arm in (ARM_A, ARM_B):
        arm_rows = [r for r in rows if r["arm"] == arm]
        if not arm_rows:
            continue
        tok = lambda k: sum((r.get("tokens") or {}).get(k, 0) for r in arm_rows)  # noqa: E731
        costs = [r.get("cost_usd") or 0.0 for r in arm_rows]
        lines.append(
            f"| {arm} | {len(arm_rows)} | {sum(1 for r in arm_rows if r['passed'])} | {sum(costs):.2f} | {_stat(costs)} | "
            f"{tok('input')} | {tok('output')} | {tok('cache_read')} | {tok('cache_write')} | {_stat([r.get('turns') for r in arm_rows])} | "
            f"{_stat([r.get('tool_search_calls', 0) for r in arm_rows])} | {_stat([r.get('baseline_prompt_tokens') for r in arm_rows])} | "
            f"{sum(1 for r in arm_rows if r.get('declared') == 'DONE')} |"
        )
    by_status: dict[tuple[str, str], int] = defaultdict(int)
    for r in rows:
        by_status[(r["arm"], r.get("status_detail") or r["status"])] += 1
    lines += [
        "",
        "Trial status by arm: "
        + ", ".join(f"{arm} {status}={n}" for (arm, status), n in sorted(by_status.items())),
    ]
    deltas = [
        (r["arm"], (r.get("quota_seven_day_after") or 0) - (r.get("quota_seven_day_before") or 0))
        for r in rows
        if r.get("quota_seven_day_after") is not None
        and r.get("quota_seven_day_before") is not None
    ]
    if deltas:
        for arm in (ARM_A, ARM_B):
            vals = [d for a, d in deltas if a == arm]
            if vals:
                lines.append(
                    f"- 7-day quota used per trial, {arm}: mean {statistics.mean(vals):.4f} over {len(vals)} trials "
                    "(the login is shared, so other sessions add noise)"
                )
    if run_dir is not None:
        pauses = run_dir / "pauses.jsonl"
        if pauses.exists():
            ends = [
                json.loads(ln)
                for ln in pauses.read_text().splitlines()
                if ln.strip() and '"event": "end"' in ln
            ]
            total = sum(e.get("actual_s", 0) for e in ends)
            lines.append(f"- pauses: {len(ends)} totalling {total / 60:.1f} min (see pauses.jsonl)")
    return "\n".join(lines) + "\n"


def retitle(markdown: str) -> str:
    return (
        markdown.replace(
            "Pilot analysis: cua-driver-mcp vs codex-native-cu",
            "Claude Code benchmark: cc-cua-driver vs cc-codex-cu",
        )
        .replace(
            "(Codex CLI + Cua Driver MCP)",
            "(Claude Code + Cua Driver 0.34.0 MCP + Cua Driver skill)",
        )
        .replace(
            "(Codex CLI + Codex built-in computer use)",
            "(Claude Code + Codex computer-use cua_repl MCP, no skill)",
        )
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("results")
    parser.add_argument("--out-md")
    parser.add_argument("--out-json")
    parser.add_argument("--include-smoke", action="store_true")
    parser.add_argument("--include-incomplete", action="store_true")
    parser.add_argument(
        "--phase1-only", action="store_true", help="primary analysis on phase-1 runs only"
    )
    parser.add_argument("--bootstrap", type=int, default=10000)
    parser.add_argument("--seed", type=int, default=1234)
    parser.add_argument("--pass-k", type=int, default=3)
    parser.add_argument("--headline-groups", default="AF")
    parser.add_argument("--coverage-tags", default="hover,coverage_probe")
    args = parser.parse_args(argv)

    analyze.ARM_A, analyze.ARM_B, analyze.ARMS = ARM_A, ARM_B, (ARM_A, ARM_B)
    raw = [
        json.loads(ln) for ln in Path(args.results).read_text("utf-8").splitlines() if ln.strip()
    ]
    rows = select_rows(raw, args.include_smoke, args.include_incomplete, args.phase1_only)
    if not rows:
        print(
            "analyze_bench.py: no analysable rows (smoke, retried and incomplete-block rows are excluded)",
            file=sys.stderr,
        )
        return 2
    pilot_rows = to_pilot_rows(rows)
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as handle:
        handle.write("".join(json.dumps(r) + "\n" for r in pilot_rows))
        tmp = handle.name
    try:
        loaded = analyze.load_rows(tmp)
        result = analyze.analyze(
            loaded,
            bootstrap=args.bootstrap,
            seed=args.seed,
            pass_k=args.pass_k,
            headline_groups=[g for g in args.headline_groups.split(",") if g],
            coverage_tags=[t for t in args.coverage_tags.split(",") if t],
        )
    except (analyze.TrialFormatError, ValueError) as error:
        print(f"analyze_bench.py: error: {error}", file=sys.stderr)
        return 2
    markdown = retitle(analyze.render_markdown(result)) + claude_section(
        rows, Path(args.results).resolve().parent
    )
    if args.out_md:
        Path(args.out_md).write_text(markdown, "utf-8")
    else:
        print(markdown)
    if args.out_json:
        Path(args.out_json).write_text(
            json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n", "utf-8"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
