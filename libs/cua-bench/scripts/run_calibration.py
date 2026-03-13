#!/usr/bin/env python3
"""Run N simultaneous cb runs per task and report pass rates.

Usage:
    python scripts/run_calibration.py --tasks "154d0750,1625e97a" [--attempts 5] [--max-steps 1000]

Output:
    scripts/tasks/calibration_results.json
"""
import argparse
import json
import os
import re
import subprocess
import time
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent
TASKS_DIR = SCRIPT_DIR / "tasks"
CB = str(SCRIPT_DIR.parent / ".venv" / "bin" / "cb")
RUNS_DIR = Path.home() / ".local" / "share" / "cua-bench" / "runs"
OUTPUT_FILE = TASKS_DIR / "calibration_results.json"

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


def extract_score(run_id: str, task_id: str) -> float | None:
    """Extract evaluation score from a run's log file."""
    log = RUNS_DIR / run_id / f"{task_id}_v0" / "run.log"
    if not log.exists():
        return None
    try:
        for line in log.read_text(errors="replace").splitlines():
            if "Evaluation result:" in line:
                return float(line.split("Evaluation result:")[-1].strip())
    except Exception:
        pass
    return None


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tasks", required=True, help="Comma-separated task IDs")
    parser.add_argument("--attempts", type=int, default=5, help="Simultaneous runs per model")
    parser.add_argument("--max-steps", type=int, default=1000)
    parser.add_argument("--output", type=Path, default=OUTPUT_FILE)
    args = parser.parse_args()

    load_env()

    task_ids = [t.strip() for t in args.tasks.split(",")]
    print(f"Tasks: {task_ids}")

    # results[task_id][model_name] = [score, ...]
    results: dict[str, dict[str, list[float]]] = {
        tid: {m: [] for m, _ in MODELS} for tid in task_ids
    }

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
                if run_id:
                    print(f"    run {i+1}: {run_id}")
                    run_ids.append(run_id)
                else:
                    print(f"    run {i+1}: FAILED to get run ID")

            wait_for_runs(run_ids)

            scores = []
            for run_id in run_ids:
                score = extract_score(run_id, task_id)
                scores.append(score if score is not None else 0.0)
                print(f"    {run_id}: {score}")

            results[task_id][model_name] = scores
            pass_rate = sum(1 for s in scores if s > 0) / len(scores) if scores else 0.0
            print(f"  [{model_name}] pass rate: {pass_rate:.0%} ({sum(1 for s in scores if s > 0)}/{len(scores)})")

    # Summary
    print("\n=== Summary ===")
    print(f"{'Task':<12} {'Claude':>10} {'OpenAI':>10}")
    print("-" * 34)
    for task_id in task_ids:
        row = results[task_id]
        for model_name, _ in MODELS:
            scores = row[model_name]
            row[model_name + "_pass_rate"] = sum(1 for s in scores if s > 0) / len(scores) if scores else 0.0
        print(f"{task_id:<12} {row['claude_pass_rate']:>9.0%} {row['openai_pass_rate']:>9.0%}")

    args.output.write_text(json.dumps(results, indent=2))
    print(f"\nWrote {args.output}")


if __name__ == "__main__":
    main()
