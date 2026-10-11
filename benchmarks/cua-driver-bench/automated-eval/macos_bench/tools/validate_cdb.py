#!/usr/bin/env python3
"""Offline validation of the CDB tasks in this runner. No model call, no GUI.

For each task: reset through the runner's adapter (the pack's setup, then its byte-level verify), run the
evaluator on the pristine workspace (a scripted wrong solution: it must fail), apply a scripted correct
solution, run the evaluator (it must pass), reset again and check the reset restores the pristine state
(the evaluator fails again). The scripted solutions are the helpers of the pack's own test modules
(`tests/test_<task>.py`, class `Workspace`), applied to the adapter's workspace.

  CDB_TASKPACK=<dir with tasks/> validate_cdb.py [--out FILE]
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import cdb_adapter  # noqa: E402

TASKS = {
    "CDB-S01": ("shared/cdb-s01", "test_cdb_s01", "complete_correctly"),
    "CDB-S02": ("shared/cdb-s02", "test_cdb_s02", "complete"),
    "CDB-S03": ("shared/cdb-s03", "test_cdb_s03", "complete"),
    "CDB-S04": ("shared/cdb-s04", "test_cdb_s04", "complete"),
}


def load_helper(task_dir: Path, module_name: str):
    spec = importlib.util.spec_from_file_location(
        module_name, task_dir / "tests" / f"{module_name}.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)  # type: ignore[union-attr]
    return module


def attach(module, workspace: Path, artifacts: Path):
    """The test module's Workspace helper, pointed at the adapter's workspace (no second setup)."""
    ws = object.__new__(module.Workspace)
    ws.root = workspace
    ws.artifacts = artifacts
    ws.repo = workspace / "seed-exchange"
    ws.store = workspace / "tickets"
    return ws


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=None)
    args = ap.parse_args()
    report: dict[str, dict] = {}
    ok_all = True
    for task_id, (pack_task, module_name, solve) in TASKS.items():
        spec = {"id": task_id, "pack_task": pack_task, "kind": "cdb"}
        with tempfile.TemporaryDirectory() as tmp:
            artifacts = Path(tmp)
            task = cdb_adapter.CdbTask(spec, artifacts)
            helper = load_helper(task.bundle, module_name)
            row: dict[str, object] = {}
            try:
                task.reset()
                row["reset_ok"] = True
                wrong = task.evaluate(0)
                row["pristine_fails"] = wrong["passed"] is False
                row["pristine_score"] = wrong.get("score")
                ws = attach(helper, task.workspace, artifacts)
                getattr(ws, solve)()
                right = task.evaluate(0)
                row["correct_passes"] = right["passed"] is True
                row["correct_score"] = right.get("score")
                task.reset()
                again = task.evaluate(0)
                row["reset_restores_pristine"] = again["passed"] is False
            except Exception as error:  # noqa: BLE001
                row["error"] = f"{type(error).__name__}: {str(error)[:300]}"
            finally:
                task.clean_workspace()
            good = (
                row.get("reset_ok")
                and row.get("pristine_fails")
                and row.get("correct_passes")
                and row.get("reset_restores_pristine")
            )
            row["ok"] = bool(good)
            ok_all &= bool(good)
            report[task_id] = row
            print(task_id, json.dumps(row))
    if args.out:
        args.out.write_text(json.dumps(report, indent=2) + "\n", "utf-8")
    return 0 if ok_all else 1


if __name__ == "__main__":
    raise SystemExit(main())
