"""Run an adapter over a dataset, save raw results, produce a summary.
`dataset_hash()` (task.py) is recorded on every run so a summary always says
exactly which frozen dataset version produced it (the pre-registration
mechanism documented in task.py's own docstring).
"""
from __future__ import annotations

import json
import time
from dataclasses import asdict
from pathlib import Path

from ..task import CuaTask, dataset_hash
from .adapter import ModelAdapter
from .diagnostics import diagnose
from .scoring import (TaskResult, accuracy, composite_score, element_accuracy,
                      expected_calibration_error, score_task)


def _strip_for_modality(task: CuaTask, modality: str) -> CuaTask:
    """A multimodal run must not see the ax tree; a text run must not see the
    screenshot -- task.py's own docstring requires this. Returns a shallow
    copy with the other modality's artifact removed."""
    if modality not in task.modality_available:
        raise ValueError(f"task {task.id} does not support modality {modality!r} "
                         f"(available: {task.modality_available})")
    d = task.to_json()
    if modality == "multimodal":
        d["ax_tree"] = None
        d["ax_tree_source"] = None
        d["modality_available"] = ["multimodal"]
    elif modality == "text":
        d["screenshot"] = None
        d["modality_available"] = ["text"]
    else:
        raise ValueError(f"unknown modality {modality!r}")
    return CuaTask.from_json(d)


def run(tasks: list[CuaTask], adapter: ModelAdapter, modality: str) -> list[TaskResult]:
    results = []
    for task in tasks:
        stripped = _strip_for_modality(task, modality)
        t0 = time.perf_counter()
        probs = adapter.predict(stripped, modality)
        latency = time.perf_counter() - t0
        results.append(score_task(task, probs, latency_s=latency))
    return results


def summarize(results: list[TaskResult], tasks: list[CuaTask], adapter_name: str, modality: str) -> dict:
    return {
        "adapter": adapter_name,
        "modality": modality,
        "n_tasks": len(results),
        "dataset_hash": dataset_hash(tasks),
        "accuracy": accuracy(results),
        "element_accuracy": element_accuracy(results),
        "ece": expected_calibration_error(results),
        "composite_score": composite_score(results),
        "n_invalid": sum(1 for r in results if not r.valid),
        "mean_latency_s": (sum(r.latency_s for r in results if r.latency_s is not None) / len(results)) if results else 0.0,
    }


def run_and_save(tasks: list[CuaTask], adapter: ModelAdapter, modality: str, out_dir: Path) -> dict:
    """Runs the adapter, writes raw per-task results to `<out_dir>/raw.jsonl`,
    the summary to `<out_dir>/summary.json`, a per-family/role/action-type
    breakdown to `<out_dir>/diagnostics.json` (see eval.diagnostics -- this is
    what would have surfaced a family- or role-isolated failure instead of
    averaging it away in one top-line accuracy number), and returns the
    summary."""
    out_dir.mkdir(parents=True, exist_ok=True)
    results = run(tasks, adapter, modality)
    with (out_dir / "raw.jsonl").open("w", encoding="utf-8") as f:
        for r in results:
            f.write(json.dumps(asdict(r), ensure_ascii=False) + "\n")
    summary = summarize(results, tasks, adapter.name, modality)
    (out_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    diagnostics = diagnose(tasks, results, modality)
    (out_dir / "diagnostics.json").write_text(json.dumps(diagnostics, indent=2), encoding="utf-8")
    return summary
