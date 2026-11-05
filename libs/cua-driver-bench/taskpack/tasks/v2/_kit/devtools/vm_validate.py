#!/usr/bin/env python3
"""VM-only validation of the v2 tasks' reset, export and evaluator, without any model (never on a workstation).

    python3 vm_validate.py PACK_V2_DIR [--tasks NA-01,NA-02,...] [--out results.json]

For every task: run its descriptor's reset setup and verify (with the app-state flag where the descriptor has
it), then its export and evaluator on the untouched state (expect fail, score 0). For the native-app tasks it
also applies the task's own scripted solution (`tests/vm_solve.py`) straight to the app's data or files (JXA, xattr,
chflags, sips: not the GUI)
and expects the evaluator to pass, then resets again and expects the reset to verify. Run it from Terminal in
the VM so the Automation consent is Terminal's, as in trials.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

KIT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(KIT))


def load_solver(task_dir: Path):
    """The task's own scripted solution (`tests/vm_solve.py`, a `solve(workspace)` function), or None."""
    path = task_dir / "tests" / "vm_solve.py"
    if not path.is_file():
        return None
    spec = importlib.util.spec_from_file_location(f"vm_solve_{task_dir.name.replace('-', '_')}", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.solve


def sub(text: str, values: dict[str, str]) -> str:
    return re.sub(r"\$\{(\w+)\}", lambda m: values.get(m.group(1), m.group(0)), text)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pack_v2", type=Path)
    ap.add_argument("--tasks", default="")
    ap.add_argument("--out", type=Path)
    args = ap.parse_args()
    wanted = {t.strip() for t in args.tasks.split(",") if t.strip()}
    report = {}
    for task_dir in sorted(p for p in args.pack_v2.iterdir() if (p / "task.v2.json").is_file()):
        meta = json.loads((task_dir / "task.v2.json").read_text())
        tid = meta["id"]
        if wanted and tid not in wanted:
            continue
        desc = json.loads((task_dir / "platform/launch.macos.json").read_text())
        sem = desc["semantics"]
        tmp = Path(tempfile.mkdtemp(prefix=f"v2val-{tid}-"))
        ws = Path(sub(desc["workspace_root"], {"HOME": str(Path.home())}))
        values = {"python": sys.executable, "workspace": str(ws), "workspace_uri": ws.as_uri(), "bundle": str(task_dir),
                  "HOME": str(Path.home()), "artifacts": str(tmp), "result": str(tmp / "result.json"), "agent_exit_code": "0"}

        def run(cmd: list[str], timeout: int = 300) -> subprocess.CompletedProcess:
            return subprocess.run([sub(c, values) for c in cmd], cwd=task_dir, capture_output=True, text=True, timeout=timeout)

        def verdict() -> dict:
            if sem.get("export"):
                e = run(sem["export"])
                if e.returncode != 0:
                    return {"export_error": e.stderr[-300:]}
            r = run(sem["evaluate"])
            if r.returncode != 0 or not (tmp / "result.json").exists():
                return {"eval_error": (r.stderr or r.stdout)[-300:]}
            data = json.loads((tmp / "result.json").read_text())
            return {"passed": data["passed"], "score": data["score"],
                    "failed": sorted(k for k, v in data["checks"].items() if not v["passed"])}

        row: dict = {"category": meta["category"]}
        t0 = time.monotonic()
        s = run(sem["reset"]["setup"])
        v = run(sem["reset"]["verify"]) if s.returncode == 0 else None
        row["reset"] = "ok" if s.returncode == 0 and v and v.returncode == 0 else f"FAIL {(s.stderr or (v.stderr if v else ''))[-300:]}"
        row["untouched"] = verdict()
        solve = load_solver(task_dir)
        if solve is not None:
            try:
                solve(ws)
                row["solved"] = verdict()
            except Exception as error:  # noqa: BLE001
                row["solved"] = {"solve_error": str(error)[-300:]}
            s = run(sem["reset"]["setup"])
            v = run(sem["reset"]["verify"]) if s.returncode == 0 else None
            row["reset_after_solve"] = "ok" if s.returncode == 0 and v and v.returncode == 0 else f"FAIL {(s.stderr or (v.stderr if v else ''))[-300:]}"
        row["seconds"] = round(time.monotonic() - t0, 1)
        report[tid] = row
        print(tid, json.dumps(row), flush=True)
    if args.out:
        args.out.write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
