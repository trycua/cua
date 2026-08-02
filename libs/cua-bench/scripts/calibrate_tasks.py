#!/usr/bin/env python3
"""Calibrate KiCad task difficulty by running each task N times per model.

Usage:
    python scripts/calibrate_tasks.py [--attempts N] [--parallel P] [--tasks-dir DIR]

Outputs:
    scripts/tasks/calibration.json  — per-task pass rates and difficulty tiers
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

SCRIPT_DIR = Path(__file__).parent
TASKS_DIR = SCRIPT_DIR / "tasks"
OUTPUT_FILE = TASKS_DIR / "calibration.json"

MODELS = [
    ("claude", "anthropic/claude-opus-4-6"),
    ("openai", "openai/computer-use-preview"),
]

DEFAULT_ATTEMPTS = 5
DEFAULT_PARALLEL = 6
DEFAULT_MAX_STEPS = 150

# Difficulty tiers (from TBench spec, based on worst-model accuracy)
# Frontier:      best model ≤ 20%
# Advanced Plus: worst model ≤ 20%   (and not Frontier)
# Advanced:      20% < worst ≤ 60%
# Core:          60% < worst ≤ 80%
# Easy:          worst > 80%


def _tier(claude_rate: float, openai_rate: float) -> str:
    best = max(claude_rate, openai_rate)
    worst = min(claude_rate, openai_rate)
    if best <= 0.20:
        return "frontier"
    if worst <= 0.20:
        return "advanced_plus"
    if worst <= 0.60:
        return "advanced"
    if worst <= 0.80:
        return "core"
    return "easy"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _load_env() -> dict[str, str]:
    env_file = SCRIPT_DIR.parent / ".env"
    if env_file.exists():
        for line in env_file.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, _, v = line.partition("=")
                key, val = k.strip(), v.strip()
                os.environ[key] = val  # export into current process too
    return os.environ.copy()


def _cb() -> str:
    return str(SCRIPT_DIR.parent / ".venv" / "bin" / "cb")


def _run_dataset(model_name: str, model_id: str, parallel: int,
                 max_steps: int, tasks_dir: Path,
                 task_ids: list[str], attempts: int) -> str:
    """Build a temp dir with `attempts` copies of each task, run as one dataset."""
    import tempfile, shutil
    tmp = Path(tempfile.mkdtemp(prefix="cb_calib_"))
    try:
        for task_id in task_ids:
            for i in range(attempts):
                dst = tmp / f"{task_id}_{i}"
                shutil.copytree(tasks_dir / task_id, dst)
        cmd = [
            _cb(), "run", "dataset", str(tmp),
            "--agent", "cua-agent",
            "--model", model_id,
            "--max-parallel", str(parallel),
            "--max-steps", str(max_steps),
        ]
        print(f"  [{model_name}] Starting {len(task_ids)} tasks × {attempts} attempts "
              f"(parallel={parallel}): {' '.join(cmd)}")
        result = subprocess.run(cmd, capture_output=True, text=True, cwd=str(SCRIPT_DIR.parent))
        for line in result.stdout.splitlines():
            if "Run ID:" in line:
                return line.split("Run ID:")[-1].strip().split()[0]
        raise RuntimeError(f"Could not parse run ID:\n{result.stdout}\n{result.stderr}")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def _wait_for_run(run_id: str, poll_interval: int = 30) -> None:
    """Poll cb run list until all sessions for this run are in a terminal state."""
    cb = _cb()
    print(f"    Waiting for run {run_id}...", end="", flush=True)
    while True:
        result = subprocess.run(
            [cb, "run", "list"],
            capture_output=True, text=True,
            cwd=str(SCRIPT_DIR.parent),
        )
        lines = [l for l in result.stdout.splitlines() if run_id[:8] in l]
        if lines:
            terminal = [l for l in lines if any(
                s in l for s in ("completed", "failed", "error", "done", "0.", "1.")
            )]
            if len(terminal) == len(lines):
                print(" done.")
                return
        print(".", end="", flush=True)
        time.sleep(poll_interval)


def _get_run_output_dir(run_id: str) -> Path | None:
    """Find the output directory for a run from cb run info."""
    cb = _cb()
    result = subprocess.run(
        [cb, "run", "info", run_id],
        capture_output=True, text=True,
        cwd=str(SCRIPT_DIR.parent),
    )
    for line in result.stdout.splitlines():
        if "Output:" in line or "output" in line.lower():
            parts = line.split(":", 1)
            if len(parts) == 2:
                p = Path(parts[1].strip())
                if p.exists():
                    return p
    # Fallback: search default location
    default = Path.home() / ".local" / "share" / "cua-bench" / "runs" / run_id
    if default.exists():
        return default
    return None


def _extract_scores(run_output_dir: Path) -> dict[str, float]:
    """Extract per-task scores from a run output directory.

    Returns {task_id: score} where score is 0.0–1.0.
    """
    scores: dict[str, float] = {}
    if not run_output_dir or not run_output_dir.exists():
        return scores

    try:
        from datasets import load_from_disk
    except ImportError:
        # Fallback: parse run.log for "Evaluation result:"
        for log in run_output_dir.rglob("run.log"):
            task_id = log.parent.name.split("_")[0]
            for line in log.read_text().splitlines():
                if "Evaluation result:" in line:
                    try:
                        scores[task_id] = float(line.split(":")[-1].strip())
                    except ValueError:
                        pass
        return scores

    for trace_dir in run_output_dir.rglob("task_*_trace"):
        session_dir = trace_dir.parent
        task_id = session_dir.name.split("_")[0]
        try:
            ds = load_from_disk(str(trace_dir))
            for row in ds:
                if row.get("event_name") == "evaluate":
                    data = json.loads(row["data_json"])
                    scores[task_id] = float(data.get("result", 0.0))
                    break
        except Exception:
            # Fallback to run.log
            log = session_dir / "run.log"
            if log.exists():
                for line in log.read_text().splitlines():
                    if "Evaluation result:" in line:
                        try:
                            scores[task_id] = float(line.split(":")[-1].strip())
                        except ValueError:
                            pass
    return scores


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Calibrate KiCad task difficulty")
    parser.add_argument("--attempts", type=int, default=DEFAULT_ATTEMPTS,
                        help=f"Number of attempts per model (default: {DEFAULT_ATTEMPTS})")
    parser.add_argument("--parallel", type=int, default=DEFAULT_PARALLEL,
                        help=f"Max parallel tasks per run (default: {DEFAULT_PARALLEL})")
    parser.add_argument("--max-steps", type=int, default=DEFAULT_MAX_STEPS,
                        help=f"Max steps per task (default: {DEFAULT_MAX_STEPS})")
    parser.add_argument("--tasks-dir", type=Path, default=TASKS_DIR)
    parser.add_argument("--output", type=Path, default=OUTPUT_FILE)
    parser.add_argument("--task-filter", type=str, default=None,
                        help="Comma-separated task IDs to run (e.g. '55f2eefb,d1c655da')")
    args = parser.parse_args()

    # Load .env into os.environ so cb and any subprocesses pick it up
    _load_env()

    # Discover task IDs (optionally filtered)
    all_task_ids = sorted(
        p.name for p in args.tasks_dir.iterdir()
        if p.is_dir() and (p / "main.py").exists()
    )
    if args.task_filter:
        patterns = [p.strip() for p in args.task_filter.split(",")]
        task_ids = [t for t in all_task_ids if any(t.startswith(p) or t == p for p in patterns)]
    else:
        task_ids = all_task_ids
    print(f"Found {len(task_ids)} tasks: {task_ids}")

    # Accumulate scores: {task_id: {model_name: [score, ...]}}
    all_scores: dict[str, dict[str, list[float]]] = {
        tid: {m: [] for m, _ in MODELS} for tid in task_ids
    }

    for model_name, model_id in MODELS:
        print(f"\n=== Model: {model_name} ({model_id}) ===")
        for attempt in range(args.attempts):
            run_id = _run_dataset(
                model_name, model_id, attempt,
                args.parallel, args.max_steps,
                args.tasks_dir, task_ids,
            )
            _wait_for_run(run_id)
            out_dir = _get_run_output_dir(run_id)
            scores = _extract_scores(out_dir)
            print(f"    Attempt {attempt+1} scores: {scores}")
            for task_id in task_ids:
                score = scores.get(task_id, 0.0)
                all_scores[task_id][model_name].append(score)

    # Compute pass rates and tiers
    results: dict[str, dict] = {}
    for task_id in task_ids:
        task_scores = all_scores[task_id]
        rates = {
            m: (sum(task_scores[m]) / len(task_scores[m]) if task_scores[m] else 0.0)
            for m, _ in MODELS
        }
        tier = _tier(rates["claude"], rates["openai"])
        results[task_id] = {
            "pass_rates": rates,
            "attempts": args.attempts,
            "difficulty": tier,
            "raw_scores": task_scores,
        }
        print(f"  {task_id}: claude={rates['claude']:.2f} openai={rates['openai']:.2f} → {tier}")

    # Summary
    tier_counts: dict[str, int] = {}
    for r in results.values():
        tier_counts[r["difficulty"]] = tier_counts.get(r["difficulty"], 0) + 1
    print(f"\nDifficulty distribution: {tier_counts}")

    # Write manifest
    manifest = {
        "version": "1.0",
        "models": {m: mid for m, mid in MODELS},
        "attempts_per_model": args.attempts,
        "tasks": results,
    }
    args.output.write_text(json.dumps(manifest, indent=2))
    print(f"\nWrote {args.output}")


if __name__ == "__main__":
    main()
