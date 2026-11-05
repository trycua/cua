#!/usr/bin/env python3
"""Run every v2 task's checker tests, each in its own process (their evaluator modules share a name).

    python3 tasks/v2/run_tests.py [--e2e]     # --e2e also replays each web task's reference solution headless

Prints one line per task and exits non-zero when any task fails.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--e2e", action="store_true")
    args = ap.parse_args()
    bad = 0
    for task in sorted(p for p in ROOT.iterdir() if (p / "task.v2.json").is_file()):
        meta = json.loads((task / "task.v2.json").read_text())
        done = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=task, capture_output=True, text=True)
        tail = (done.stderr or done.stdout).strip().splitlines()
        line = f"{meta['id']:6} {meta['category']:13} unit {'ok' if done.returncode == 0 else 'FAIL'} ({tail[-3] if len(tail) >= 3 else ''})"
        ok = done.returncode == 0
        if args.e2e and (task / "tests" / "solution.json").is_file():
            runner = ROOT / "_kit" / "devtools" / "run_solution.py"
            solved = subprocess.run([sys.executable, str(runner), str(task)], capture_output=True, text=True)
            untouched = subprocess.run([sys.executable, str(runner), str(task), "--steps", "/dev/stdin", "--expect", "fail"],
                                       input='[{"wait": 300}]', capture_output=True, text=True)
            ok = ok and solved.returncode == 0 and untouched.returncode == 0
            line += f"; e2e solved {'pass' if solved.returncode == 0 else 'FAIL'}, untouched {'fail as expected' if untouched.returncode == 0 else 'WRONG'}"
        bad += not ok
        print(line, flush=True)
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
