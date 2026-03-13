#!/usr/bin/env python3
"""Run N simultaneous cb runs per task for calibration.

Usage:
    python scripts/run_calibration.py --tasks "154d0750,1625e97a" [--attempts 5] [--max-steps 1000]
"""
import argparse
import os
import re
import subprocess
import time
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent
TASKS_DIR = SCRIPT_DIR / "tasks"
CB = str(SCRIPT_DIR.parent / ".venv" / "bin" / "cb")

MODELS = [
    ("claude", "anthropic/claude-opus-4-6"),
    ("openai", "openai/computer-use-preview"),
]


def load_env() -> None:
    env_file = SCRIPT_DIR.parent / ".env"
    if env_file.exists():
        for line in env_file.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, _, v = line.partition("=")
                os.environ[k.strip()] = v.strip()


def start_run(task_path: Path, model_id: str, max_steps: int) -> str:
    """Start one cb run dataset for a single task, return its run ID."""
    result = subprocess.run(
        [CB, "run", "dataset", str(task_path.parent),
         "--agent", "cua-agent", "--model", model_id, "--max-steps", str(max_steps),
         "--max-parallel", "1", "--task-filter", task_path.name],
        capture_output=True, text=True, cwd=str(SCRIPT_DIR.parent),
    )
    clean = re.sub(r"\x1b\[[0-9;]*m", "", result.stdout)
    for line in clean.splitlines():
        if "Run ID:" in line:
            return line.split("Run ID:")[-1].strip().split()[0]
    raise RuntimeError(f"No run ID in output:\n{result.stdout}\n{result.stderr}")


def wait_for_runs(run_ids: list[str]) -> None:
    """Block until all run IDs reach a terminal state."""
    pending = set(run_ids)
    print(f"    Waiting for {len(pending)} runs...", end="", flush=True)
    while pending:
        result = subprocess.run(
            [CB, "run", "list"], capture_output=True, text=True, cwd=str(SCRIPT_DIR.parent),
        )
        for run_id in list(pending):
            for line in result.stdout.splitlines():
                if run_id[:8] in line and any(s in line for s in ("completed", "failed", "error")):
                    pending.discard(run_id)
        if pending:
            print(".", end="", flush=True)
            time.sleep(20)
    print(" done.")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tasks", required=True, help="Comma-separated task IDs")
    parser.add_argument("--attempts", type=int, default=5, help="Simultaneous runs per model")
    parser.add_argument("--max-steps", type=int, default=1000)
    args = parser.parse_args()

    load_env()

    task_ids = [t.strip() for t in args.tasks.split(",")]
    print(f"Tasks: {task_ids}")

    for task_id in task_ids:
        task_path = TASKS_DIR / task_id
        if not task_path.exists():
            print(f"WARNING: {task_path} not found, skipping")
            continue

        print(f"\n=== Task: {task_id} ===")
        for model_name, model_id in MODELS:
            print(f"  [{model_name}] Launching {args.attempts} simultaneous runs...")
            run_ids = []
            for i in range(args.attempts):
                run_id = start_run(task_path, model_id, args.max_steps)
                print(f"    run {i+1}: {run_id}")
                if run_id:
                    run_ids.append(run_id)
                else:
                    print(f"    run {i+1}: FAILED to get run ID")
            wait_for_runs(run_ids)
            print(f"  [{model_name}] done.")


if __name__ == "__main__":
    main()
