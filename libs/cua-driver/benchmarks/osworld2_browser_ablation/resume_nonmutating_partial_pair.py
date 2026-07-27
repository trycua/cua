#!/usr/bin/env python3
"""Resume a sealed pair whose first arm failed after nonmutating actions."""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from openai import OpenAI

import run_paired_gpt55 as paired
import run_persistent_web_certification as persistent


class ResumeError(RuntimeError):
    pass


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ResumeError(f"{path} must contain an object")
    return value


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--partial-paired-result", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--container-disk-image", required=True)
    parser.add_argument("--env-file", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--model", default="gpt-5.5")
    parser.add_argument("--reasoning-effort", default="xhigh")
    parser.add_argument("--max-steps", type=int, default=24)
    parser.add_argument("--cpu-cores", type=int, default=2)
    parser.add_argument("--memory", default="8Gi")
    parser.add_argument("--max-estimated-cost-usd", type=float, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    partial = read_json(args.partial_paired_result)
    episodes = partial.get("episodes") or []
    if (
        partial.get("pair_valid") is not False
        or partial.get("order") != "treatment-first"
        or len(episodes) != 1
        or episodes[0].get("mode") != "combined"
    ):
        raise ResumeError("partial result is not an eligible treatment-first pair")
    task_id = str(partial.get("task_id"))
    paired.configure_task(task_id)
    prior_episode_dir = (
        args.partial_paired_result.parent / "episodes" / "01-combined"
    )
    prior_result, _, _ = paired.load_nonmutating_resume_trace(
        prior_episode_dir,
        expected_mode="combined",
    )
    if int(prior_result.get("steps_executed") or 0) >= args.max_steps:
        raise ResumeError("partial arm has no remaining steps")
    if args.output_dir.exists():
        raise ResumeError("--output-dir must not already exist")
    if args.max_estimated_cost_usd <= 0:
        raise ResumeError("--max-estimated-cost-usd must be positive")
    args.output_dir.mkdir(parents=True)

    client = OpenAI(
        api_key=paired.require_api_key(args.env_file),
        timeout=paired.OPENAI_TIMEOUT_SECONDS,
    )
    fleet_log = args.output_dir / "fleet-pilot.log"
    started_at = time.time()
    process, log_stream = paired.start_pilot(
        args.config,
        fleet_log,
        args.container_disk_image,
        cpu_cores=args.cpu_cores,
        memory=args.memory,
    )
    live: dict[str, Any] | None = None
    cleanup: dict[str, Any] | None = None
    results: list[dict[str, Any]] = []
    run_error: str | None = None
    try:
        live = paired.wait_for_pilot(process, started_at)
        osworld = paired.verify_osworld_provenance()
        provenance_args = SimpleNamespace(
            model=args.model,
            reasoning_effort=args.reasoning_effort,
            max_steps=args.max_steps,
            order="treatment-first",
            setup_only=False,
        )
        provenance = paired.provenance(
            container_disk_image=args.container_disk_image,
            live=live,
            args=provenance_args,
            osworld_provenance=osworld,
        )
        provenance["resume_runner_sha256"] = paired.sha256_file(Path(__file__))
        provenance["superseded_partial_result"] = {
            "path": str(args.partial_paired_result.resolve()),
            "sha256": paired.sha256_file(args.partial_paired_result),
        }
        paired.write_json(args.output_dir / "provenance.json", provenance)

        combined_cache = args.output_dir / "task-cache" / "01-combined"
        combined_reset = paired.reset_and_setup_task(combined_cache)
        combined = paired.run_episode(
            client=client,
            model=args.model,
            reasoning_effort=args.reasoning_effort,
            mode="combined",
            max_steps=args.max_steps,
            episode_dir=args.output_dir / "episodes" / "01-combined-resumed",
            cache_dir=combined_cache,
            reset_evidence=combined_reset,
            cost_ceiling_usd=args.max_estimated_cost_usd,
            resume_from_episode=prior_episode_dir,
        )
        results.append(combined)
        if combined.get("agent_failure"):
            raise ResumeError(str(combined["agent_failure"]))

        remaining = args.max_estimated_cost_usd - persistent.result_cost(
            {"episodes": results}
        )
        if remaining <= 0:
            raise ResumeError("resumed treatment exhausted the pair cost ceiling")
        control_cache = args.output_dir / "task-cache" / "02-screenshot_ax"
        control_reset = paired.reset_and_setup_task(control_cache)
        control = paired.run_episode(
            client=client,
            model=args.model,
            reasoning_effort=args.reasoning_effort,
            mode="screenshot_ax",
            max_steps=args.max_steps,
            episode_dir=args.output_dir / "episodes" / "02-screenshot_ax",
            cache_dir=control_cache,
            reset_evidence=control_reset,
            cost_ceiling_usd=remaining,
        )
        results.append(control)
        if control.get("agent_failure"):
            raise ResumeError(str(control["agent_failure"]))
    except Exception as exc:
        run_error = f"{type(exc).__name__}: {exc}"
    finally:
        try:
            paired.stop_pilot(process)
        finally:
            log_stream.close()
        try:
            cleanup = (
                paired.pilot_cleanup_record(live)
                if live is not None
                else paired.pilot_cleanup_record_from_log(fleet_log)
            )
        except Exception as exc:
            cleanup_error = f"{type(exc).__name__}: {exc}"
            run_error = (
                f"{run_error}; cleanup verification failed: {cleanup_error}"
                if run_error
                else f"cleanup verification failed: {cleanup_error}"
            )

    result = persistent.build_paired_result(
        task_id=task_id,
        requested_model=args.model,
        order="treatment-first",
        results=results,
        cleanup=cleanup,
        run_error=run_error,
    )
    result["supersedes_result"] = {
        "path": str(args.partial_paired_result.resolve()),
        "sha256": paired.sha256_file(args.partial_paired_result),
        "reason": (
            "The first arm failed on guest transport after three replayable, "
            "nonmutating actions. Those actions were replayed on an official "
            "fresh reset and the arm continued from step 4 without repeating "
            "the first three model calls."
        ),
    }
    result["total_estimated_cost_includes_superseded_partial"] = True
    paired.write_json(args.output_dir / "paired-result.json", result)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["pair_valid"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ResumeError as exc:
        print(json.dumps({"status": "error", "message": str(exc)}))
        raise SystemExit(1)
