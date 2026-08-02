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
         "--max-parallel", "5", "--task-filter", task_path.name],
        capture_output=True, text=True, cwd=str(SCRIPT_DIR.parent),
    )
    clean = re.sub(r"\x1b\[[0-9;]*m", "", result.stdout)
    for line in clean.splitlines():
        if "Run ID:" in line:
            return line.split("Run ID:")[-1].strip().split()[0]
    raise RuntimeError(f"No run ID in output:\n{result.stdout}\n{result.stderr}")


def kill_containers() -> None:
    """Stop and remove any lingering cua-* Docker containers."""
    result = subprocess.run(
        ["docker", "ps", "-q", "--filter", "name=cua-"],
        capture_output=True, text=True,
    )
    ids = result.stdout.strip().split() if result.stdout.strip() else []
    if ids:
        subprocess.run(["docker", "stop", "--time", "10"] + ids,
                       capture_output=True)
        subprocess.run(["docker", "rm", "-f"] + ids,
                       capture_output=True)


def wait_for_runs(run_ids: list[str]) -> None:
    """Block until all run IDs reach a terminal state, then kill any lingering containers."""
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


def is_server_error(run_id: str, task_id: str) -> bool:
    """Return True if the run failed due to a remote 500/server error."""
    log = RUNS_DIR / run_id / f"{task_id}_v0" / "run.log"
    if not log.exists():
        return False
    text = log.read_text(errors="replace")
    return "500 Internal Server Error" in text or "InternalServerError" in text


def print_summary(results: dict, task_ids: list[str], output: Path) -> None:
    """Print running summary table and flush results to disk."""
    print("\n=== Summary so far ===")
    print(f"{'Task':<12} {'Claude':>10} {'OpenAI':>10}")
    print("-" * 34)
    for task_id in task_ids:
        row = results[task_id]
        rates = {}
        for model_name, _ in MODELS:
            scores = row.get(model_name, [])
            rates[model_name] = sum(1 for s in scores if s > 0) / len(scores) if scores is not None and len(scores) > 0 else None
        claude = f"{rates['claude']:.0%}" if rates["claude"] is not None else "pending"
        openai = f"{rates['openai']:.0%}" if rates["openai"] is not None else "pending"
        print(f"{task_id:<12} {claude:>10} {openai:>10}")
    output.write_text(json.dumps(results, indent=2))
    print(f"(results written to {output})\n")


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

        # Launch all models simultaneously
        model_run_ids: dict[str, list[str]] = {model_name: [] for model_name, _ in MODELS}
        for model_name, model_id in MODELS:
            print(f"  [{model_name}] Launching {args.attempts} simultaneous runs...")
            for i in range(args.attempts):
                try:
                    run_id = start_run(task_path, model_id, args.max_steps)
                    print(f"    [{model_name}] run {i+1}: {run_id}")
                    model_run_ids[model_name].append(run_id)
                except Exception as e:
                    print(f"    [{model_name}] run {i+1}: FAILED to start — {e}")

        # Wait for all runs across all models
        all_run_ids = [rid for rids in model_run_ids.values() for rid in rids]
        wait_for_runs(all_run_ids)

        # Retry any OpenAI runs that failed with a server 500 error
        retry_ids = []
        for model_name, model_id in MODELS:
            if model_name != "openai":
                continue
            for i, run_id in enumerate(model_run_ids[model_name]):
                if is_server_error(run_id, task_id):
                    print(f"    [openai] {run_id}: 500 server error — retrying...")
                    try:
                        new_id = start_run(task_path, model_id, args.max_steps)
                        print(f"    [openai] retry run: {new_id}")
                        model_run_ids[model_name][i] = new_id
                        retry_ids.append(new_id)
                    except Exception as e:
                        print(f"    [openai] retry FAILED to start — {e}")
        if retry_ids:
            wait_for_runs(retry_ids)

        # Extract scores per model
        for model_name, _ in MODELS:
            scores = []
            for run_id in model_run_ids[model_name]:
                score = extract_score(run_id, task_id)
                scores.append(score if score is not None else 0.0)
                print(f"    [{model_name}] {run_id}: {score}")
            results[task_id][model_name] = scores
            pass_rate = sum(1 for s in scores if s > 0) / len(scores) if scores else 0.0
            print(f"  [{model_name}] pass rate: {pass_rate:.0%} ({sum(1 for s in scores if s > 0)}/{len(scores)})")

        print_summary(results, task_ids, args.output)


if __name__ == "__main__":
    main()
