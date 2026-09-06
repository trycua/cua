#!/usr/bin/env python3
"""Run the sealed OSWorld V2 web certification tranche one VM at a time."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
DEFAULT_MANIFEST = ROOT / "manifests" / "web_certification_tranche.json"
DEFAULT_WORK_DIR = Path(
    __import__("os").environ.get("CUA_OSWORLD2_WORK_DIR", ROOT / ".work")
).expanduser().resolve()


class CertificationError(RuntimeError):
    pass


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise CertificationError(f"{path.name} must contain an object")
    return value


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def episode_cost(result: dict[str, Any]) -> float:
    return sum(
        float((episode.get("cost") or {}).get("estimated_usd") or 0.0)
        for episode in result.get("episodes") or []
        if isinstance(episode, dict)
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--config", type=Path, default=DEFAULT_WORK_DIR / "local.json")
    parser.add_argument("--container-disk-image", required=True)
    parser.add_argument("--env-file", type=Path, required=True)
    parser.add_argument("--model", default="gpt-5.5")
    parser.add_argument("--reasoning-effort", default="xhigh")
    parser.add_argument("--max-steps", type=int, default=24)
    parser.add_argument("--output-dir", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    manifest = read_json(args.manifest)
    tasks = manifest.get("tasks")
    if not isinstance(tasks, list) or len(tasks) != 10:
        raise CertificationError("certification manifest must contain ten tasks")
    cost_cap = float(manifest["max_estimated_model_cost_usd"])
    minimum_rate = float(manifest["minimum_valid_pair_rate"])
    run_id = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    output_dir = (
        args.output_dir
        or DEFAULT_WORK_DIR / "results" / f"web-certification-{run_id}"
    )
    output_dir.mkdir(parents=True, exist_ok=False)

    records: list[dict[str, Any]] = []
    spent = 0.0
    stopped_reason: str | None = None
    for index, task in enumerate(tasks):
        if not isinstance(task, dict):
            raise CertificationError("task manifest entries must be objects")
        task_id = str(task["task_id"])
        tasks_remaining = len(tasks) - index
        remaining_budget = cost_cap - spent
        if remaining_budget <= 0:
            stopped_reason = "estimated model-cost ceiling reached"
            break
        invalid_count = sum(not item["pair_valid"] for item in records)
        allowed_invalid = len(tasks) - int(minimum_rate * len(tasks))
        if invalid_count > allowed_invalid:
            stopped_reason = "minimum valid-pair rate is no longer attainable"
            break

        pair_cap = remaining_budget / tasks_remaining
        pair_dir = output_dir / f"{index + 1:02d}-task{task_id}"
        log_path = output_dir / f"{index + 1:02d}-task{task_id}.log"
        command = [
            sys.executable,
            str(ROOT / "run_paired_gpt55.py"),
            "--config",
            str(args.config),
            "--container-disk-image",
            args.container_disk_image,
            "--task-id",
            task_id,
            "--model",
            args.model,
            "--reasoning-effort",
            args.reasoning_effort,
            "--max-steps",
            str(args.max_steps),
            "--max-estimated-cost-usd",
            f"{pair_cap:.8f}",
            "--env-file",
            str(args.env_file),
            "--output-dir",
            str(pair_dir),
            "--order",
            str(task["order"]),
        ]
        with log_path.open("w", encoding="utf-8") as stream:
            completed = subprocess.run(
                command,
                cwd=ROOT,
                stdout=stream,
                stderr=subprocess.STDOUT,
                text=True,
                check=False,
            )
        result_path = pair_dir / "paired-result.json"
        result = read_json(result_path) if result_path.is_file() else {}
        cost = episode_cost(result)
        spent += cost
        record = {
            "task_id": task_id,
            "stratum": task["stratum"],
            "order": task["order"],
            "pair_cap_usd": pair_cap,
            "runner_exit_code": completed.returncode,
            "pair_valid": result.get("pair_valid") is True,
            "estimated_model_cost_usd": cost,
            "result_path": str(result_path),
            "log_path": str(log_path),
            "run_error": result.get("run_error"),
            "pair_validation_errors": result.get("pair_validation_errors") or [],
            "fleet_cleanup": result.get("fleet_cleanup"),
        }
        records.append(record)
        write_json(
            output_dir / "certification-summary.json",
            {
                "schema_version": 1,
                "manifest": str(args.manifest.resolve()),
                "model": args.model,
                "reasoning_effort": args.reasoning_effort,
                "max_steps": args.max_steps,
                "cost_cap_usd": cost_cap,
                "estimated_model_cost_usd": spent,
                "completed_tasks": len(records),
                "valid_pairs": sum(item["pair_valid"] for item in records),
                "records": records,
                "stopped_reason": stopped_reason,
            },
        )

    valid = sum(item["pair_valid"] for item in records)
    final_rate = valid / len(tasks)
    passed = (
        len(records) == len(tasks)
        and final_rate >= minimum_rate
        and spent <= cost_cap
    )
    summary = {
        "schema_version": 1,
        "manifest": str(args.manifest.resolve()),
        "model": args.model,
        "reasoning_effort": args.reasoning_effort,
        "max_steps": args.max_steps,
        "cost_cap_usd": cost_cap,
        "estimated_model_cost_usd": spent,
        "completed_tasks": len(records),
        "valid_pairs": valid,
        "valid_pair_rate": final_rate,
        "passed": passed,
        "records": records,
        "stopped_reason": stopped_reason,
    }
    write_json(output_dir / "certification-summary.json", summary)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CertificationError as exc:
        print(json.dumps({"status": "error", "message": str(exc)}))
        raise SystemExit(1)
