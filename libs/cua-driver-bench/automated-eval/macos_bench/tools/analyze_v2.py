#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Bench v2 report (Amendment 14, A14.7): per arm, overall and per category.

    tools/analyze_v2.py RUN_DIR [RUN_DIR ...] [--include-smoke] [--json OUT.json] [--md OUT.md]

Reads `results.jsonl` of each run (final rows of bench_version 2 only; infrastructure exclusions
dropped). For every arm, overall and per category:

* success: passed / trials with a Wilson 95% interval (a GUI-only violation is a fail, A14.2);
* mean turns, equivalent cost and wall time per trial;
* background operation (A14.4): background-clean trials, trials with a focus steal and the number
  of steals, trials where the real pointer moved, windows raised, input that leaked into the
  witness; and the headline form "34/50 passed, 0 focus steals";
* GUI-only (A14.2): violations, with each trial's evidence listed;
* interruptions (A14.5, IR-* only): shown, exercised, handled and completed rates;
* shared desktop (SHARED-DESKTOP.md): PA-01 per number of copies (all copies passed, copies passed, cross-talk,
  makespan, throughput, speed-up against N = 1 paired by run index), UM-01 (checker pass, full pass, the mouse
  user's displacement and clicks).

Rows written by the Windows and Hyprland session launchers (`row.json`) are read with `--session-rows DIR`.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import bench_v2 as v2  # noqa: E402
from analyze import wilson_interval  # noqa: E402

# Public setup names; any other arm is reported under its arm id.
ARM_LABELS = {
    "cc-cua-driver-script": "Cua Driver, driven by Claude Code",  # suffix from Amendment 23
    "cc-codex-cu": "Codex Computer-Use (Sky), driven by Claude Code",  # suffix from Amendment 20
    "cc-codex-cu-1007-browser": "Codex Computer-Use (Sky) 26.1007",
    "cx-codex-cu": "Codex Computer-Use (Sky), driven by Codex CLI",  # Amendment 20; the rows' model is appended
    "cx-cua-driver": "Cua Driver, driven by Codex CLI",  # Amendment 23; the rows' model is appended
    "cx-claude-cu-helper": (  # Amendment 23; the rows' model is appended
        "Claude Computer-Use (Desktop helper): Claude Desktop 2.31226.0 computer-use helper via a minimal"
        " adapter; an internal interface, not an Anthropic-supported procedure; driven by Codex CLI"
    ),
    "cc-claude-cu-helper": (
        "Claude Computer-Use (Desktop helper): Claude Desktop 2.31226.0 computer-use helper via a minimal"
        " adapter; an internal interface, not an Anthropic-supported procedure"
    ),
}


def load_rows(runs: list[Path], include_smoke: bool = False) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for run in runs:
        path = run / "results.jsonl" if run.is_dir() else run
        for line in path.read_text("utf-8").splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            if (
                not row.get("final")
                or row.get("excluded")
                or row.get("bench_version") != v2.BENCH_VERSION
            ):
                continue
            if row.get("smoke") and not include_smoke:
                continue
            rows.append(row)
    return rows


def load_session_rows(dirs: list[Path]) -> list[dict[str, Any]]:
    """Rows of launch/windows_session.py and launch/hyprland_session.py (`row.json` under each DIR), mapped to
    the run_bench row fields this report reads. Rows with a harness error are dropped (re-run, never scored)."""
    rows: list[dict[str, Any]] = []
    for d in dirs:
        for path in sorted(d.rglob("row.json")):
            row = json.loads(path.read_text("utf-8"))
            if row.get("harness_error") or "passed" not in row:
                continue
            spec_path = HERE / "probes" / str(row.get("task")) / "task.json"
            spec = json.loads(spec_path.read_text("utf-8")) if spec_path.is_file() else {}
            evaluation = row.get("evaluation") or {}
            rows.append(
                {
                    **row,
                    "arm": row.get("label") or row.get("platform") or "session",
                    "trial_id": str(path.parent.relative_to(d)),
                    "category": spec.get("category"),
                    "final": True,
                    "bench_version": v2.BENCH_VERSION,
                    "score": evaluation.get("score"),
                    "multiplayer": row.get("a22"),
                    "background_condition": row.get("a15"),
                    "agent_wall_s": row.get("agent_wall_s"),
                }
            )
    return rows


def _mean(values: list[float]) -> float | None:
    values = [v for v in values if isinstance(v, (int, float))]
    return round(sum(values) / len(values), 3) if values else None


def _rate(k: int, n: int) -> dict[str, Any]:
    ci = wilson_interval(k, n)
    return {
        "k": k,
        "n": n,
        "rate": round(k / n, 3) if n else None,
        "ci95": [round(x, 3) for x in ci] if ci else None,
    }


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    n = len(rows)
    passed = sum(1 for r in rows if r.get("passed"))
    bg = [r.get("background") or {} for r in rows]
    measured = [b for b in bg if b.get("measured")]
    gui = [r.get("gui_only") or {} for r in rows]
    return {
        "trials": n,
        "success": _rate(passed, n),
        "passed_raw": sum(1 for r in rows if r.get("passed_raw")),
        "turns_mean": _mean([r.get("turns") for r in rows]),
        "cost_usd_mean": _mean([r.get("cost_usd") for r in rows]),
        "wall_s_mean": _mean([r.get("agent_wall_s") or r.get("wall_s") for r in rows]),
        "background": {
            "measured": len(measured),
            "clean": _rate(sum(1 for b in measured if b.get("clean")), len(measured)),
            "trials_with_focus_steal": sum(1 for b in measured if b.get("focus_steals")),
            "focus_steals": sum(int(b.get("focus_steals") or 0) for b in measured),
            "trials_pointer_moved": sum(1 for b in measured if b.get("pointer_moved")),
            "windows_raised": sum(int(b.get("windows_raised") or 0) for b in measured),
            "trials_window_raised": sum(1 for b in measured if b.get("windows_raised")),
            "windows_measured": sum(1 for b in measured if b.get("windows_measured")),
            "input_leaked": sum(int(b.get("input_leaked") or 0) for b in measured),
        },
        "gui_only_violations": sum(1 for g in gui if g.get("violation")),
        "background_condition": _bgc(rows),
        "multiplayer": _mp(rows),
    }


def _bgc(rows: list[dict[str, Any]]) -> dict[str, Any]:
    """Amendment 15: the primary metric of the background condition and its secondaries."""
    items = [r.get("background_condition") or {} for r in rows]
    bg_rows = [i for i in items if i.get("condition") == "background"]
    return {
        "background_completion": _rate(
            sum(1 for i in bg_rows if i.get("background_completion")), len(bg_rows)
        )
        if bg_rows
        else None,
        "background_completion_pointer_still": _rate(
            sum(1 for i in bg_rows if i.get("background_completion_pointer_still")), len(bg_rows)
        )
        if bg_rows
        else None,
        "user_window_held": sum(1 for i in items if i.get("user_window_held")),
        "user_input_disrupted": sum(1 for i in items if i.get("user_input_disrupted")),
        "overlay_owners": sorted({o for i in items for o in (i.get("overlay_owners") or [])}),
    }


def _mp_of(row: dict[str, Any]) -> dict[str, Any]:
    """A22 score of a row: as recorded, or re-expressed (labelled "A22 rescored") for rows from before A22."""
    if row.get("multiplayer"):
        return row["multiplayer"]
    measured = bool((row.get("background") or {}).get("measured"))
    return v2.multiplayer_score(
        row.get("disturbance") or {},
        measured,
        bool(row.get("passed")),
        row.get("condition"),
        rescore=True,
    )


def _mp(rows: list[dict[str, Any]]) -> dict[str, Any] | None:
    """Amendment 22: the headline background metric, and how many background trials broke each criterion."""
    items = [_mp_of(r) for r in rows if r.get("condition") == "background"]
    if not items:
        return None
    return {
        "background_completion": _rate(
            sum(1 for i in items if i.get("background_completion")), len(items)
        ),
        "rules": sorted({i["rule"] for i in items}),
        "disturbed": {
            k: sum(1 for i in items if i["criteria"].get(k) is False)
            for k in v2.MULTIPLAYER_CRITERIA
        },
        "unmeasured": {
            k: sum(1 for i in items if i["criteria"].get(k) is None)
            for k in v2.MULTIPLAYER_CRITERIA
        },
    }


def interruptions(rows: list[dict[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    by_task: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for r in rows:
        if r.get("interruption"):
            by_task[r["task"]].append(r["interruption"])
    for task, items in sorted(by_task.items()):
        shown = [i for i in items if i.get("shown")]
        out[task] = {
            "kind": items[0].get("kind"),
            "trials": len(items),
            "shown": len(shown),
            "exercised": sum(1 for i in items if i.get("exercised")),
            "handled": _rate(sum(1 for i in shown if i.get("handled")), len(shown)),
            "completed": _rate(sum(1 for i in items if i.get("completed")), len(items)),
        }
    return out


def _n_of(row: dict[str, Any]) -> int:
    return int(row.get("instances") or (row.get("parallel") or {}).get("instances") or 0)


def shared_desktop(rows: list[dict[str, Any]]) -> dict[str, Any] | None:
    """PA-01 per number of copies and UM-01 (SHARED-DESKTOP.md). Speed-up is throughput at N over throughput at
    N = 1 for the same run index (same seed), averaged over the pairs that have both."""
    pa = [r for r in rows if r.get("task") == "PA-01" and r.get("parallel")]
    um = [r for r in rows if r.get("task") == "UM-01"]
    if not pa and not um:
        return None
    out: dict[str, Any] = {"PA-01": {}, "UM-01": None}
    base = {
        r.get("run_index"): r["parallel"].get("throughput_per_min") for r in pa if _n_of(r) == 1
    }
    for n in sorted({_n_of(r) for r in pa}):
        sub = [r for r in pa if _n_of(r) == n]
        par = [r["parallel"] for r in sub]
        ratios = [
            p.get("throughput_per_min") / base[r.get("run_index")]
            for r, p in zip(sub, par)
            if base.get(r.get("run_index"))
            and isinstance(p.get("throughput_per_min"), (int, float))
        ]
        out["PA-01"][str(n)] = {
            "trials": len(sub),
            "all_copies_passed": _rate(sum(1 for r in sub if r.get("passed")), len(sub)),
            "copies_passed_mean": _mean([p.get("passed") for p in par]),
            "trials_with_cross_talk": sum(1 for p in par if p.get("cross_talk")),
            "cross_talk_copies": sum(int(p.get("cross_talk") or 0) for p in par),
            "makespan_s_mean": _mean([p.get("makespan_s") for p in par]),
            "throughput_per_min_mean": _mean([p.get("throughput_per_min") for p in par]),
            "speedup_vs_1_mean": _mean(ratios) if n != 1 else 1.0,
            "speedup_pairs": len(ratios) if n != 1 else None,
            "full_pass_background": _rate(
                sum(1 for r in sub if (_mp_of(r) or {}).get("background_completion")), len(sub)
            )
            if all(r.get("condition") == "background" for r in sub)
            else None,
        }
    if um:
        mus = [((r.get("disturbance") or {}).get("mouse_user") or {}) for r in um]
        out["UM-01"] = {
            "trials": len(um),
            "checker_passed": _rate(sum(1 for r in um if r.get("passed")), len(um)),
            "full_pass": _rate(
                sum(1 for r in um if (_mp_of(r) or {}).get("background_completion")), len(um)
            ),
            "trials_pointer_displaced": sum(
                1
                for m in mus
                if float(m.get("max_displacement_px") or 0) > float(m.get("tolerance_px") or 3)
            ),
            "max_displacement_px": max(
                (float(m.get("max_displacement_px") or 0) for m in mus), default=None
            ),
            "trials_with_lost_clicks": sum(
                1 for m in mus if int(m.get("clicks_landed") or 0) < int(m.get("clicks") or 0)
            ),
            "user_clicks": sum(int(m.get("clicks") or 0) for m in mus),
            "user_clicks_not_landed": sum(
                int(m.get("clicks") or 0) - int(m.get("clicks_landed") or 0) for m in mus
            ),
            "foreign_pointer_events": sum(int(m.get("foreign_pointer_events") or 0) for m in mus),
            "tagging": sorted({str(m.get("tagging")) for m in mus}),
            "agent_clicks_in_user_window": sum(
                int((r.get("disturbance") or {}).get("clicks_leaked") or 0) for r in um
            ),
        }
    return out


def analyze(rows: list[dict[str, Any]]) -> dict[str, Any]:
    by_arm: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for r in rows:  # Amendment 15: one block per arm and condition
        by_arm[f"{r['arm']}|{r.get('condition') or 'foreground'}"].append(r)
    report: dict[str, Any] = {
        "bench_version": v2.BENCH_VERSION,
        "categories": v2.CATEGORIES,
        "arms": {},
    }
    for key, arm_rows in sorted(by_arm.items()):
        arm, condition = key.split("|")
        cats = {}
        for cat in v2.CATEGORIES:
            sub = [r for r in arm_rows if r.get("category") == cat]
            cats[cat] = summarize(sub) if sub else None
        report["arms"][key] = {
            "arm": arm,
            "condition": condition,
            "label": ARM_LABELS.get(arm, f"{arm} (internal baseline)")
            + (
                f" + {', '.join(sorted({str(r.get('model')) for r in arm_rows}))}"
                if arm.startswith("cx-")
                else ""
            )
            + f" [{condition}]",
            "overall": summarize(arm_rows),
            "by_category": cats,
            "interruptions": interruptions(arm_rows),
            "shared_desktop": shared_desktop(arm_rows),
            "violations": [
                {"trial_id": r["trial_id"], "evidence": (r.get("gui_only") or {}).get("violations")}
                for r in arm_rows
                if (r.get("gui_only") or {}).get("violation")
            ],
        }
    return report


def headline(s: dict[str, Any]) -> str:
    b = s["background"]
    mp = (s.get("multiplayer") or {}).get("background_completion")
    if mp:  # A22: the background condition leads with multi-player background completion
        return f"{mp['k']}/{mp['n']} completed with the user undisturbed, {s['success']['k']}/{s['trials']} passed"
    bc = (s.get("background_condition") or {}).get("background_completion")
    if bc:  # A15.3: the background condition leads with background completion
        return f"{bc['k']}/{bc['n']} completed in the background, {s['success']['k']}/{s['trials']} passed, {b['focus_steals']} focus steals"
    return f"{s['success']['k']}/{s['trials']} passed, {b['focus_steals']} focus steals"


def _frac(rate: dict[str, Any] | None) -> str:
    return f"{rate['k']}/{rate['n']}" if rate else "n/a"


def _fmt(x: Any) -> str:
    return "n/a" if x is None else (f"{x:.2f}" if isinstance(x, float) else str(x))


def markdown(report: dict[str, Any]) -> str:
    lines = ["# Bench v2 results (Amendment 14)", ""]
    lines += [
        "## Overall",
        "",
        "| Setup | Headline | Success (95% CI) | Background-clean | Pointer moved | Windows raised | GUI-only violations | Turns | Cost | Wall s |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for arm, a in report["arms"].items():
        s = a["overall"]
        b = s["background"]
        ci = s["success"]["ci95"]
        lines.append(
            f"| {a['label']} | {headline(s)} | {_fmt(s['success']['rate'])} ({ci[0]:.2f}–{ci[1]:.2f}) | "
            f"{b['clean']['k']}/{b['measured']} | {b['trials_pointer_moved']} | {b['windows_raised']} | "
            f"{s['gui_only_violations']} | {_fmt(s['turns_mean'])} | {_fmt(s['cost_usd_mean'])} | {_fmt(s['wall_s_mean'])} |"
            if ci
            else f"| {a['label']} | no trials | | | | | | | | |"
        )
    for cat, label in report["categories"].items():
        lines += [
            "",
            f"## {label}",
            "",
            "| Setup | Passed | Background-clean | Focus steals | Turns | Cost | Wall s |",
            "|---|---|---|---|---|---|---|",
        ]
        for arm, a in report["arms"].items():
            s = a["by_category"].get(cat)
            if not s:
                lines.append(f"| {a['label']} | no tasks | | | | | |")
                continue
            b = s["background"]
            lines.append(
                f"| {a['label']} | {s['success']['k']}/{s['trials']} | {b['clean']['k']}/{b['measured']} | "
                f"{b['focus_steals']} | {_fmt(s['turns_mean'])} | {_fmt(s['cost_usd_mean'])} | {_fmt(s['wall_s_mean'])} |"
            )
    lines += [
        "",
        "## Interruptions (IR probes)",
        "",
        "| Setup | Task | Kind | Shown | Exercised | Handled | Completed |",
        "|---|---|---|---|---|---|---|",
    ]
    for arm, a in report["arms"].items():
        for task, i in a["interruptions"].items():
            lines.append(
                f"| {a['label']} | {task} | {i['kind']} | {i['shown']}/{i['trials']} | {i['exercised']}/{i['trials']} | "
                f"{i['handled']['k']}/{i['handled']['n']} | {i['completed']['k']}/{i['completed']['n']} |"
            )
    lines += [
        "",
        "## Background condition (Amendment 15)",
        "",
        "| Setup | Background completion | Success regardless of focus | Focus steals | User window held | User input disrupted | Overlays (metadata) |",
        "|---|---|---|---|---|---|---|",
    ]
    for arm, a in report["arms"].items():
        s = a["overall"]
        bc = s["background_condition"]
        if a["condition"] != "background":
            continue
        c = bc["background_completion"]
        lines.append(
            f"| {a['label']} | {c['k']}/{c['n']} | {s['success']['k']}/{s['trials']} | {s['background']['focus_steals']} | "
            f"{bc['user_window_held']}/{s['trials']} | {bc['user_input_disrupted']} | {', '.join(bc['overlay_owners']) or 'none'} |"
        )
    cols = list(v2.MULTIPLAYER_CRITERIA)
    lines += [
        "",
        "## Background completion with the user undisturbed (Amendment 22, headline)",
        "",
        "Completed = passed and none of the eight disturbances below happened. Each column counts the background "
        'trials in which that disturbance happened. Rows marked "A22 rescored" come from runs before A22 and are scored on the criteria those runs measured (secondary figures).',
        "",
        "| Setup | Rule | Background completion | "
        + " | ".join(v2.MULTIPLAYER_CRITERIA[c].capitalize() for c in cols)
        + " |",
        "|---|---|---|" + "---|" * len(cols),
    ]
    for arm, a in report["arms"].items():
        mp = a["overall"].get("multiplayer")
        if a["condition"] != "background" or not mp:
            continue
        c = mp["background_completion"]
        n = mp["background_completion"]["n"]
        cells = [
            "not measured"
            if mp["unmeasured"][k] == n
            else f"{mp['disturbed'][k]}"
            + (f" ({mp['unmeasured'][k]} not measured)" if mp["unmeasured"][k] else "")
            for k in cols
        ]
        lines.append(
            f"| {a['label']} | {', '.join(mp['rules'])} | {c['k']}/{c['n']} | "
            + " | ".join(cells)
            + " |"
        )
    if any(a.get("shared_desktop") for a in report["arms"].values()):
        lines += [
            "",
            "## Shared desktop: parallel agents (PA-01)",
            "",
            "All copies passed = every copy's checks held (code, shelf, one Submit, no other copy's code letters, in "
            "the budget). Speed-up = throughput at N over throughput at N = 1, same run index.",
            "",
            "| Setup | N | All copies passed | Copies passed (mean) | Trials with cross-talk | Makespan s | "
            "Copies per minute | Speed-up vs N = 1 | Full pass (background) |",
            "|---|---|---|---|---|---|---|---|---|",
        ]
        for a in report["arms"].values():
            sdk = (a.get("shared_desktop") or {}).get("PA-01") or {}
            for n, p in sdk.items():
                c = p["all_copies_passed"]
                fp = p.get("full_pass_background")
                lines.append(
                    f"| {a['label']} | {n} | {c['k']}/{c['n']} | {_fmt(p['copies_passed_mean'])} | "
                    f"{p['trials_with_cross_talk']} | {_fmt(p['makespan_s_mean'])} | "
                    f"{_fmt(p['throughput_per_min_mean'])} | {_fmt(p['speedup_vs_1_mean'])} | "
                    f"{_frac(fp)} |"
                )
        lines += [
            "",
            "## Shared desktop: the user's mouse keeps working (UM-01)",
            "",
            "Full pass = checker passed, no GUI-only violation and the eight undisturbed criteria, with the pointer "
            "compared against the user's own script and every user click on its dot.",
            "",
            "| Setup | Checker passed | Full pass | Trials pointer displaced | Max displacement px | "
            "Trials with lost clicks | User clicks not landed | Foreign pointer events | Agent clicks in user window |",
            "|---|---|---|---|---|---|---|---|---|",
        ]
        for a in report["arms"].values():
            u = (a.get("shared_desktop") or {}).get("UM-01")
            if not u:
                continue
            lines.append(
                f"| {a['label']} | {u['checker_passed']['k']}/{u['checker_passed']['n']} | "
                f"{u['full_pass']['k']}/{u['full_pass']['n']} | {u['trials_pointer_displaced']} | "
                f"{_fmt(u['max_displacement_px'])} | {u['trials_with_lost_clicks']} | "
                f"{u['user_clicks_not_landed']}/{u['user_clicks']} | {u['foreign_pointer_events']} | "
                f"{u['agent_clicks_in_user_window']} |"
            )
    viol = [(a["label"], v) for a in report["arms"].values() for v in a["violations"]]
    lines += ["", "## GUI-only violations", ""]
    lines += [f"- {label}: {v['trial_id']}: {v['evidence']}" for label, v in viol] or ["None."]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("runs", nargs="*", type=Path)
    p.add_argument(
        "--session-rows",
        nargs="+",
        type=Path,
        default=[],
        help="directories of Windows or Hyprland session trials (each with a row.json)",
    )
    p.add_argument("--include-smoke", action="store_true")
    p.add_argument("--json", type=Path, default=None)
    p.add_argument("--md", type=Path, default=None)
    args = p.parse_args(argv)
    report = analyze(
        load_rows(args.runs, args.include_smoke) + load_session_rows(args.session_rows)
    )
    text = markdown(report)
    if args.json:
        args.json.write_text(json.dumps(report, indent=2) + "\n", "utf-8")
    if args.md:
        args.md.write_text(text, "utf-8")
    sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
