# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Validation stand-ins for the agent in a Windows pack-task trial (CDB-G02 to CDB-G04). Never a bench setup.

  cdb_oracle.py --task CDB-G02 --workspace {workspace}             scripted correct solution: the checker passes
  cdb_oracle.py --task CDB-G02 --workspace {workspace} --noop      does nothing: the checker fails, user undisturbed
  cdb_oracle.py --task CDB-G02 --workspace {workspace} --disturb   one deliberate disturbance the sentinel must see

The scripted solution is the helper of the pack's own test module (`tests/test_<task>.py`, class `Workspace`,
method `complete`) applied to the trial's workspace, as `tools/validate_cdb.py` does on macOS. It writes the
end state to the workspace files directly; it checks the evaluator and the Windows lifecycle, not the GUI.
"""

from __future__ import annotations

import argparse
import ctypes
import importlib.util
import json
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import cdb as C  # noqa: E402

PROBES = HERE.parent / "probes"


def solve(task: str, workspace: Path) -> None:
    pack_task = json.loads((PROBES / task / "task.json").read_text("utf-8"))["pack_task"]
    bundle = C.TASKPACK / "tasks" / pack_task
    name = "test_" + Path(pack_task).name.replace("-", "_")
    spec = importlib.util.spec_from_file_location(name, bundle / "tests" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)  # type: ignore[union-attr]
    ws = object.__new__(module.Workspace)  # pointed at the trial's workspace, no second setup
    ws.root = workspace
    ws.artifacts = Path(tempfile.mkdtemp(prefix="cdb-oracle-"))
    ws.complete()


def disturb(task: str) -> None:
    """Bring one of the task's windows to the front, move the real pointer and clear the clipboard."""
    import winenv as W

    pack_task = json.loads((PROBES / task / "task.json").read_text("utf-8"))["pack_task"]
    descriptor = json.loads(C.descriptor_path(pack_task).read_text("utf-8"))
    time.sleep(2)
    wins = [a["window"] for a in descriptor["apps"] if a.get("window")]
    for hwnd in W.top_level_windows():
        if W.shown(hwnd) and any(C.title_matches(w, W.text(hwnd)) for w in wins):
            W.force_foreground(hwnd)  # focus steal, activation change, raise over the user window
            break
    time.sleep(1)
    W.user32.SetCursorPos(300, 300)  # real pointer movement
    ctypes.windll.user32.OpenClipboard(0)
    ctypes.windll.user32.EmptyClipboard()  # clipboard change
    ctypes.windll.user32.CloseClipboard()
    time.sleep(2)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--task", required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--noop", action="store_true")
    parser.add_argument("--disturb", action="store_true")
    args = parser.parse_args()
    if args.disturb:
        disturb(args.task)
    elif args.noop:
        time.sleep(3)
    else:
        solve(args.task, args.workspace)
        time.sleep(2.0)  # the sentinel needs 2 s of samples to measure the user window
    time.sleep(1.0)


if __name__ == "__main__":
    main()
