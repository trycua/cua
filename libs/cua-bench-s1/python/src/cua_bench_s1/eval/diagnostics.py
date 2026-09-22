"""Per-family / per-modality / per-role / per-action-type diagnostics.

Why this exists: a single top-line accuracy number hides exactly the class of
bug that costs real time in practice -- a failure isolated to one task family
/ element role / action type, invisible in an aggregate score, that can get
mistaken for a model regression across multiple retraining attempts before a
one-off manual diagnostic proves the model innocent. This module makes that
breakdown a permanent, reusable part of the eval harness instead of something
reinvented under pressure each time.

Two granularities, both computed from the same (tasks, results) pair:
  - task-level: accuracy/ECE grouped by task family (and, across a merged
    multi-run report, by modality).
  - element-level: accuracy grouped by element role and by gold action type,
    plus a full gold-action -> predicted-action confusion matrix -- this is
    what would surface "confident wrong `skip`" as an obvious off-diagonal
    cell instead of an averaged-away aggregate.
"""
from __future__ import annotations

from collections import defaultdict

from ..task import CuaTask
from .scoring import TaskResult, expected_calibration_error

# Sentinel for an element whose predicted/gold action couldn't be determined
# (result was invalid, so per_element_predicted/gold are empty dicts).
_UNKNOWN = "<invalid>"


def _bucket_metrics(results: list[TaskResult]) -> dict:
    n = len(results)
    if n == 0:
        return {"n_tasks": 0, "accuracy": None, "ece": None}
    acc = sum(1 for r in results if r.correct) / n
    return {"n_tasks": n, "accuracy": acc, "ece": expected_calibration_error(results)}


def diagnose(tasks: list[CuaTask], results: list[TaskResult], modality: str) -> dict:
    """Build a diagnostics report for one (dataset, adapter, modality) run.
    `tasks` must be the ORIGINAL (unstripped) tasks passed to
    `cua_bench_s1.eval.runner.run` -- family/role/expected-action metadata
    comes from them, never from the modality-stripped copy the adapter saw.
    `results` must be `runner.run`'s output over the same tasks, same order.
    """
    if len(tasks) != len(results):
        raise ValueError(f"tasks ({len(tasks)}) and results ({len(results)}) length mismatch")

    tasks_by_family: dict[str, list[TaskResult]] = defaultdict(list)
    role_results: dict[str, list[bool]] = defaultdict(list)
    action_results: dict[str, list[bool]] = defaultdict(list)
    confusion: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    element_role: dict[str, dict[str, str]] = {}  # task_id -> {eid: role}

    for task in tasks:
        element_role[task.id] = {opt.element_id: opt.role for opt in task.options}

    for task, result in zip(tasks, results):
        tasks_by_family[task.family].append(result)
        roles = element_role[task.id]
        for eid, role in roles.items():
            correct = result.per_element_correct.get(eid, False)
            role_results[role].append(correct)
            gold = result.per_element_gold.get(eid, _UNKNOWN) if result.valid else _UNKNOWN
            predicted = result.per_element_predicted.get(eid, _UNKNOWN) if result.valid else _UNKNOWN
            if gold != _UNKNOWN:
                action_results[gold].append(correct)
                confusion[gold][predicted] += 1
            else:
                confusion[_UNKNOWN][_UNKNOWN] += 1

    by_family = {family: _bucket_metrics(rs) for family, rs in sorted(tasks_by_family.items())}
    by_role = {
        role: {"n_elements": len(flags), "accuracy": sum(flags) / len(flags) if flags else None}
        for role, flags in sorted(role_results.items())
    }
    by_action = {
        action: {"n_elements": len(flags), "accuracy": sum(flags) / len(flags) if flags else None}
        for action, flags in sorted(action_results.items())
    }
    confusion_out = {gold: dict(sorted(preds.items())) for gold, preds in sorted(confusion.items())}

    return {
        "modality": modality,
        "n_tasks": len(tasks),
        "overall_accuracy": sum(1 for r in results if r.correct) / len(results) if results else None,
        "overall_ece": expected_calibration_error(results) if results else None,
        "by_family": by_family,
        "by_element_role": by_role,
        "by_gold_action": by_action,
        "confusion_gold_vs_predicted": confusion_out,
    }


def merge_diagnostics(reports: list[dict]) -> dict:
    """Combine several single-modality `diagnose()` reports (e.g. one for
    "text", one for "multimodal" over the same dataset) into one dict keyed
    by modality, so a diagnostics.json can show the modality breakdown
    without forcing every caller into a multi-run API."""
    return {r["modality"]: r for r in reports}


def format_report(report: dict) -> str:
    """Human-readable rendering of one diagnose() report, for CLI/log output.
    Lines are sorted so the biggest accuracy gaps float to the top of each
    section -- an outlier family/role/action should be visible at a glance,
    not require scanning an alphabetized table."""
    lines = [f"modality={report['modality']}  n_tasks={report['n_tasks']}  "
            f"accuracy={_fmt(report['overall_accuracy'])}  ece={_fmt(report['overall_ece'])}"]

    def section(title: str, data: dict, n_key: str) -> None:
        lines.append(f"\n{title}:")
        rows = sorted(data.items(), key=lambda kv: (kv[1]["accuracy"] if kv[1]["accuracy"] is not None else 1.0))
        for key, m in rows:
            lines.append(f"  {key:<24} n={m[n_key]:<6} accuracy={_fmt(m['accuracy'])}"
                        + (f" ece={_fmt(m['ece'])}" if "ece" in m else ""))

    section("By task family", report["by_family"], "n_tasks")
    section("By element role", report["by_element_role"], "n_elements")
    section("By gold action", report["by_gold_action"], "n_elements")

    lines.append("\nConfusion (gold action -> predicted action counts):")
    for gold, preds in report["confusion_gold_vs_predicted"].items():
        pred_str = ", ".join(f"{p}={c}" for p, c in preds.items())
        lines.append(f"  gold={gold:<8} {pred_str}")

    return "\n".join(lines)


def _fmt(x: float | None) -> str:
    return "n/a" if x is None else f"{x:.3f}"
