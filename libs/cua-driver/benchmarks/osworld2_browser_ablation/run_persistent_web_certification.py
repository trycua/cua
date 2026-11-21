#!/usr/bin/env python3
"""Continue the sealed web tranche on one persistent Fleet VM.

This runner adopts completed paired results from an existing certification
directory, provisions one Fleet VM for the remaining tasks, and performs the
official OSWorld reset before every episode. It is a capacity-safe fallback
for environments where allocating a fresh VM for every pair dominates the
benchmark wall time.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from openai import OpenAI

import run_paired_gpt55 as paired
import run_web_certification as certification


ROOT = Path(__file__).resolve().parent


class PersistentCertificationError(RuntimeError):
    pass


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        default=certification.DEFAULT_MANIFEST,
    )
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--container-disk-image", required=True)
    parser.add_argument("--env-file", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--model", default="gpt-5.5")
    parser.add_argument("--reasoning-effort", default="xhigh")
    parser.add_argument("--max-steps", type=int, default=24)
    parser.add_argument("--cpu-cores", type=int, default=2)
    parser.add_argument("--memory", default="8Gi")
    parser.add_argument(
        "--quarantine-task",
        action="append",
        default=[],
        help=(
            "Preserve a model-bearing task result but exclude it from valid "
            "pairs because post-run attestation found an evaluator defect"
        ),
    )
    parser.add_argument(
        "--revalidation-evidence",
        action="append",
        type=Path,
        default=[],
        help=(
            "Trust a post-hoc score revalidation only after verifying its "
            "task, paired-result hash, setup-certification hash, and cleanup"
        ),
    )
    parser.add_argument(
        "--max-new-tasks-per-vm",
        type=int,
        help=(
            "Stop and clean up after this many newly executed task pairs, so "
            "a later invocation can continue on a fresh VM"
        ),
    )
    return parser.parse_args()


def result_cost(result: dict[str, Any]) -> float:
    return sum(
        float((episode.get("cost") or {}).get("estimated_usd") or 0.0)
        for episode in result.get("episodes") or []
        if isinstance(episode, dict)
    )


def result_has_model_attempt(result: dict[str, Any]) -> bool:
    return any(
        isinstance(episode, dict)
        and (
            int(episode.get("steps_executed") or 0) > 0
            or bool(episode.get("model_records"))
        )
        for episode in result.get("episodes") or []
    )


def build_paired_result(
    *,
    task_id: str,
    requested_model: str,
    order: str,
    results: list[dict[str, Any]],
    cleanup: dict[str, Any] | None,
    run_error: str | None,
) -> dict[str, Any]:
    by_mode = {result["mode"]: result for result in results}
    validation_errors = paired.validate_pair(results)
    pair_valid = not validation_errors and run_error is None
    return {
        "schema_version": 1,
        "task_id": task_id,
        "requested_model": requested_model,
        "order": order,
        "episodes": results,
        "pair_valid": pair_valid,
        "pair_validation_errors": validation_errors,
        "initial_official_score": (
            by_mode["screenshot_ax"]["reset_evidence"]["initial_evaluation"]["score"]
            if pair_valid
            else None
        ),
        "score_delta_combined_minus_control": (
            by_mode["combined"]["evaluation"]["score"]
            - by_mode["screenshot_ax"]["evaluation"]["score"]
            if pair_valid
            else None
        ),
        "score_gain_delta_combined_minus_control": (
            by_mode["combined"]["score_gain_from_initial"]
            - by_mode["screenshot_ax"]["score_gain_from_initial"]
            if pair_valid
            else None
        ),
        "steps_delta_combined_minus_control": (
            by_mode["combined"]["steps_executed"]
            - by_mode["screenshot_ax"]["steps_executed"]
            if pair_valid
            else None
        ),
        "wall_seconds_delta_combined_minus_control": (
            by_mode["combined"]["wall_seconds"]
            - by_mode["screenshot_ax"]["wall_seconds"]
            if pair_valid
            else None
        ),
        "model_seconds_delta_combined_minus_control": (
            by_mode["combined"]["model_seconds"]
            - by_mode["screenshot_ax"]["model_seconds"]
            if pair_valid
            else None
        ),
        "estimated_cost_usd_by_mode": (
            {
                mode: by_mode[mode]["cost"]["estimated_usd"]
                for mode in paired.MODES
            }
            if pair_valid
            else None
        ),
        "estimated_cost_usd_delta_combined_minus_control": (
            by_mode["combined"]["cost"]["estimated_usd"]
            - by_mode["screenshot_ax"]["cost"]["estimated_usd"]
            if pair_valid
            else None
        ),
        "total_estimated_cost_usd": (
            sum(
                by_mode[mode]["cost"]["estimated_usd"]
                for mode in paired.MODES
            )
            if pair_valid
            else None
        ),
        "fleet_cleanup": cleanup,
        "run_error": run_error,
        "interpretation": (
            "one task in a sealed ten-task paired certification; the Fleet VM "
            "is shared across tasks and the official OSWorld reset runs before "
            "every episode"
        ),
    }


def existing_pair_results(
    *,
    output_dir: Path,
    tasks: list[dict[str, Any]],
) -> tuple[dict[str, tuple[Path, dict[str, Any]]], list[dict[str, Any]]]:
    adopted: dict[str, tuple[Path, dict[str, Any]]] = {}
    infrastructure_attempts: list[dict[str, Any]] = []
    known_task_ids = {str(task["task_id"]) for task in tasks}
    candidates = [
        (result_path, certification.read_json(result_path))
        for result_path in sorted(output_dir.glob("**/paired-result.json"))
    ]
    superseded: dict[Path, Path] = {}
    for result_path, result in candidates:
        supersedes = result.get("supersedes_result")
        if not isinstance(supersedes, dict):
            continue
        prior_path = Path(str(supersedes.get("path"))).resolve()
        if not prior_path.is_relative_to(output_dir.resolve()):
            raise PersistentCertificationError(
                f"{result_path} supersedes a result outside the certification"
            )
        if not prior_path.is_file():
            raise PersistentCertificationError(
                f"{result_path} supersedes a missing result"
            )
        if supersedes.get("sha256") != paired.sha256_file(prior_path):
            raise PersistentCertificationError(
                f"{result_path} superseded-result hash does not match"
            )
        if prior_path in superseded:
            raise PersistentCertificationError(
                f"multiple results supersede {prior_path}"
            )
        superseded[prior_path] = result_path.resolve()

    for result_path, result in candidates:
        task_id = str(result.get("task_id"))
        if task_id not in known_task_ids:
            raise PersistentCertificationError(
                f"{result_path} has an unknown task_id"
            )
        if result_path.resolve() in superseded:
            infrastructure_attempts.append(
                {
                    "task_id": task_id,
                    "result_path": str(result_path),
                    "run_error": result.get("run_error"),
                    "superseded_by": str(superseded[result_path.resolve()]),
                }
            )
            continue
        if result.get("pair_valid") is True or result_has_model_attempt(result):
            if task_id in adopted:
                raise PersistentCertificationError(
                    f"multiple model-bearing results exist for task {task_id}"
                )
            adopted[task_id] = (result_path, result)
        else:
            infrastructure_attempts.append(
                {
                    "task_id": task_id,
                    "result_path": str(result_path),
                    "run_error": result.get("run_error"),
                }
            )
    return adopted, infrastructure_attempts


def record_for(
    *,
    task: dict[str, Any],
    pair_cap: float | None,
    result_path: Path,
    result: dict[str, Any],
    lifecycle: str,
    attestation_errors: list[str] | None = None,
    posthoc_revalidation: dict[str, Any] | None = None,
) -> dict[str, Any]:
    attestation_errors = attestation_errors or []
    pair_valid = result.get("pair_valid") is True and not attestation_errors
    return {
        "task_id": str(task["task_id"]),
        "stratum": task["stratum"],
        "order": task["order"],
        "pair_cap_usd": pair_cap,
        "runner_exit_code": 0 if pair_valid else 1,
        "pair_valid": pair_valid,
        "estimated_model_cost_usd": result_cost(result),
        "result_path": str(result_path),
        "log_path": None,
        "run_error": result.get("run_error"),
        "pair_validation_errors": [
            *(result.get("pair_validation_errors") or []),
            *attestation_errors,
        ],
        "posthoc_attestation_errors": attestation_errors,
        "posthoc_revalidation": posthoc_revalidation,
        "fleet_cleanup": result.get("fleet_cleanup"),
        "fleet_lifecycle": lifecycle,
    }


def load_revalidations(
    *,
    evidence_paths: list[Path],
    adopted: dict[str, tuple[Path, dict[str, Any]]],
    known_task_ids: set[str],
) -> dict[str, dict[str, Any]]:
    revalidations: dict[str, dict[str, Any]] = {}
    for evidence_path in evidence_paths:
        evidence = certification.read_json(evidence_path)
        task_id = str(evidence.get("task_id"))
        if task_id not in known_task_ids:
            raise PersistentCertificationError(
                f"{evidence_path} revalidates unknown task {task_id}"
            )
        if task_id not in adopted:
            raise PersistentCertificationError(
                f"{evidence_path} has no adopted paired result for task {task_id}"
            )
        if task_id in revalidations:
            raise PersistentCertificationError(
                f"multiple revalidations supplied for task {task_id}"
            )
        if evidence.get("revalidated") is not True:
            raise PersistentCertificationError(
                f"{evidence_path} is not a successful revalidation"
            )

        adopted_path, _ = adopted[task_id]
        paired_evidence = evidence.get("paired_result") or {}
        if Path(str(paired_evidence.get("path"))).resolve() != adopted_path.resolve():
            raise PersistentCertificationError(
                f"{evidence_path} points to a different paired result"
            )
        if paired_evidence.get("sha256") != paired.sha256_file(adopted_path):
            raise PersistentCertificationError(
                f"{evidence_path} paired-result hash does not match"
            )

        setup_evidence = evidence.get("setup_certification") or {}
        setup_path = Path(str(setup_evidence.get("path")))
        if not setup_path.is_file():
            raise PersistentCertificationError(
                f"{evidence_path} setup certification is missing"
            )
        if setup_evidence.get("sha256") != paired.sha256_file(setup_path):
            raise PersistentCertificationError(
                f"{evidence_path} setup-certification hash does not match"
            )
        setup = certification.read_json(setup_path)
        if not (
            setup.get("setup_valid") is True
            and setup.get("fleet_cleanup")
            and (setup["fleet_cleanup"].get("record") or {}).get(
                "cleanup_verified"
            )
            is True
        ):
            raise PersistentCertificationError(
                f"{evidence_path} setup certification or cleanup is invalid"
            )

        revalidations[task_id] = {
            "evidence_path": str(evidence_path.resolve()),
            "evidence_sha256": paired.sha256_file(evidence_path),
            "corrected_official_baseline_score": evidence.get(
                "corrected_official_baseline_score"
            ),
            "equivalence": evidence.get("equivalence"),
        }
    return revalidations


def write_summary(
    *,
    args: argparse.Namespace,
    manifest: dict[str, Any],
    tasks: list[dict[str, Any]],
    records: list[dict[str, Any]],
    infrastructure_attempts: list[dict[str, Any]],
    persistent_lifecycle: dict[str, Any],
    stopped_reason: str | None,
    final: bool,
) -> dict[str, Any]:
    cost_cap = float(manifest["max_estimated_model_cost_usd"])
    minimum_rate = float(manifest["minimum_valid_pair_rate"])
    spent = sum(float(record["estimated_model_cost_usd"]) for record in records)
    valid = sum(bool(record["pair_valid"]) for record in records)
    valid_rate = valid / len(tasks)
    passed = (
        len(records) == len(tasks)
        and valid_rate >= minimum_rate
        and spent <= cost_cap
        and persistent_lifecycle.get("cleanup_verified") is True
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
        "valid_pair_rate": valid_rate,
        "passed": passed if final else False,
        "records": records,
        "infrastructure_attempts": infrastructure_attempts,
        "persistent_lifecycle": persistent_lifecycle,
        "stopped_reason": stopped_reason,
    }
    certification.write_json(args.output_dir / "certification-summary.json", summary)
    return summary


def main() -> int:
    args = parse_args()
    manifest = certification.read_json(args.manifest)
    raw_tasks = manifest.get("tasks")
    if not isinstance(raw_tasks, list) or len(raw_tasks) != 10:
        raise PersistentCertificationError(
            "certification manifest must contain ten tasks"
        )
    tasks = [task for task in raw_tasks if isinstance(task, dict)]
    if len(tasks) != len(raw_tasks):
        raise PersistentCertificationError("task entries must be objects")
    if args.max_steps <= 0:
        raise PersistentCertificationError("--max-steps must be positive")
    if args.cpu_cores <= 0:
        raise PersistentCertificationError("--cpu-cores must be positive")
    if not args.memory.strip():
        raise PersistentCertificationError("--memory must be non-empty")
    if (
        args.max_new_tasks_per_vm is not None
        and args.max_new_tasks_per_vm <= 0
    ):
        raise PersistentCertificationError(
            "--max-new-tasks-per-vm must be positive"
        )
    if "@sha256:" not in args.container_disk_image:
        raise PersistentCertificationError(
            "--container-disk-image must pin the image by digest"
        )
    if not args.output_dir.is_dir():
        raise PersistentCertificationError(
            "--output-dir must be an existing certification directory"
        )

    adopted, infrastructure_attempts = existing_pair_results(
        output_dir=args.output_dir,
        tasks=tasks,
    )
    known_task_ids = {str(task["task_id"]) for task in tasks}
    revalidations = load_revalidations(
        evidence_paths=args.revalidation_evidence,
        adopted=adopted,
        known_task_ids=known_task_ids,
    )
    quarantine_reason = (
        "post-run attestation found that the evaluator environment omitted "
        "the official runtime controller; the numeric score is not trusted"
    )
    quarantines: dict[str, list[str]] = {}
    for task_id in args.quarantine_task:
        if task_id not in known_task_ids:
            raise PersistentCertificationError(
                f"cannot quarantine unknown task {task_id}"
            )
        if task_id not in adopted:
            raise PersistentCertificationError(
                f"cannot quarantine task {task_id} without a model-bearing result"
            )
        quarantines[task_id] = [quarantine_reason]
    overlap = set(quarantines) & set(revalidations)
    if overlap:
        raise PersistentCertificationError(
            "a task cannot be both quarantined and revalidated: "
            + ", ".join(sorted(overlap))
        )
    initial_summary_path = args.output_dir / "certification-summary.json"
    initial_summary = (
        certification.read_json(initial_summary_path)
        if initial_summary_path.is_file()
        else {}
    )
    initial_records_by_task = {
        str(record.get("task_id")): record
        for record in initial_summary.get("records") or []
        if isinstance(record, dict)
    }
    adopted_caps = {
        str(record.get("task_id")): record.get("pair_cap_usd")
        for record in initial_summary.get("records") or []
        if isinstance(record, dict)
    }
    pending = [
        task for task in tasks if str(task["task_id"]) not in adopted
    ]
    if not pending:
        persistent_lifecycle = initial_summary.get("persistent_lifecycle")
        if not isinstance(persistent_lifecycle, dict):
            raise PersistentCertificationError(
                "completed certification has no persistent lifecycle evidence"
            )
        records = []
        for task in tasks:
            task_id = str(task["task_id"])
            path, result = adopted[task_id]
            prior_record = initial_records_by_task.get(task_id) or {}
            records.append(
                record_for(
                    task=task,
                    pair_cap=adopted_caps.get(task_id),
                    result_path=path,
                    result=result,
                    lifecycle=str(
                        prior_record.get("fleet_lifecycle") or "isolated_vm"
                    ),
                    attestation_errors=quarantines.get(task_id),
                    posthoc_revalidation=revalidations.get(task_id),
                )
            )
        summary = write_summary(
            args=args,
            manifest=manifest,
            tasks=tasks,
            records=records,
            infrastructure_attempts=infrastructure_attempts,
            persistent_lifecycle=persistent_lifecycle,
            stopped_reason=None,
            final=True,
        )
        print(json.dumps(summary, indent=2, sort_keys=True))
        return 0 if summary["passed"] else 1

    client = OpenAI(
        api_key=paired.require_api_key(args.env_file),
        timeout=paired.OPENAI_TIMEOUT_SECONDS,
    )
    run_id = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    continuation_dir = args.output_dir / f"persistent-continuation-{run_id}"
    continuation_dir.mkdir(parents=False, exist_ok=False)
    fleet_log_path = continuation_dir / "fleet-pilot.log"
    started_at = time.time()
    process, log_stream = paired.start_pilot(
        args.config,
        fleet_log_path,
        args.container_disk_image,
        cpu_cores=args.cpu_cores,
        memory=args.memory,
    )
    live: dict[str, Any] | None = None
    cleanup: dict[str, Any] | None = None
    lifecycle_error: str | None = None
    new_results: dict[str, tuple[Path, dict[str, Any], float]] = {}
    stopped_reason: str | None = None
    persistent_lifecycle: dict[str, Any] = {
        "mode": "one_vm_official_reset_before_every_episode",
        "vm_shape": {
            "cpu_cores": args.cpu_cores,
            "memory": args.memory,
        },
        "fleet_log_path": str(fleet_log_path),
        "cleanup_verified": False,
    }

    try:
        live = paired.wait_for_pilot(process, started_at)
        persistent_lifecycle.update(
            {
                "namespace": live.get("namespace"),
                "driver": live.get("driver"),
                "ready_at_unix": time.time(),
            }
        )
        for task in pending:
            completed_count = len(adopted) + len(new_results)
            spent = sum(result_cost(value[1]) for value in adopted.values())
            spent += sum(result_cost(value[1]) for value in new_results.values())
            remaining_budget = (
                float(manifest["max_estimated_model_cost_usd"]) - spent
            )
            tasks_remaining = len(tasks) - completed_count
            if remaining_budget <= 0:
                stopped_reason = "estimated model-cost ceiling reached"
                break
            invalid_count = sum(
                result.get("pair_valid") is not True or task_id in quarantines
                for task_id, (_, result) in adopted.items()
            )
            invalid_count += sum(
                result.get("pair_valid") is not True
                for _, result, _ in new_results.values()
            )
            allowed_invalid = len(tasks) - int(
                float(manifest["minimum_valid_pair_rate"]) * len(tasks)
            )
            if invalid_count > allowed_invalid:
                stopped_reason = "minimum valid-pair rate is no longer attainable"
                break

            task_id = str(task["task_id"])
            paired.configure_task(task_id)
            pair_cap = remaining_budget / tasks_remaining
            pair_dir = continuation_dir / f"task{task_id}"
            pair_dir.mkdir(parents=False, exist_ok=False)
            osworld_provenance = paired.verify_osworld_provenance()
            provenance_args = SimpleNamespace(
                model=args.model,
                reasoning_effort=args.reasoning_effort,
                max_steps=args.max_steps,
                order=task["order"],
                setup_only=False,
            )
            provenance = paired.provenance(
                container_disk_image=args.container_disk_image,
                live=live,
                args=provenance_args,
                osworld_provenance=osworld_provenance,
            )
            provenance["fleet_lifecycle"] = persistent_lifecycle["mode"]
            provenance["continuation_runner_sha256"] = paired.sha256_file(
                Path(__file__)
            )
            paired.write_json(pair_dir / "provenance.json", provenance)

            results: list[dict[str, Any]] = []
            run_error: str | None = None
            try:
                modes = (
                    list(paired.MODES)
                    if task["order"] == "control-first"
                    else list(reversed(paired.MODES))
                )
                for attempt, mode in enumerate(modes, start=1):
                    cache_dir = pair_dir / "task-cache" / f"{attempt:02d}-{mode}"
                    reset_evidence = paired.reset_and_setup_task(cache_dir)
                    pair_spent = sum(
                        float(item["cost"]["estimated_usd"]) for item in results
                    )
                    remaining_pair_cost = pair_cap - pair_spent
                    if remaining_pair_cost <= 0:
                        raise paired.PairedRunError(
                            "paired run reached its estimated model-cost ceiling"
                        )
                    result = paired.run_episode(
                        client=client,
                        model=args.model,
                        reasoning_effort=args.reasoning_effort,
                        mode=mode,
                        max_steps=args.max_steps,
                        episode_dir=pair_dir
                        / "episodes"
                        / f"{attempt:02d}-{mode}",
                        cache_dir=cache_dir,
                        reset_evidence=reset_evidence,
                        cost_ceiling_usd=remaining_pair_cost,
                    )
                    results.append(result)
                    if "cost ceiling" in str(result.get("agent_failure") or ""):
                        raise paired.PairedRunError(
                            "paired run stopped at its estimated model-cost ceiling"
                        )
            except Exception as exc:
                run_error = f"{type(exc).__name__}: {exc}"

            pair_result = build_paired_result(
                task_id=task_id,
                requested_model=args.model,
                order=str(task["order"]),
                results=results,
                cleanup={"shared_lifecycle_pending": True},
                run_error=run_error,
            )
            result_path = pair_dir / "paired-result.json"
            paired.write_json(result_path, pair_result)
            new_results[task_id] = (result_path, pair_result, pair_cap)

            records: list[dict[str, Any]] = []
            for manifest_task in tasks:
                manifest_task_id = str(manifest_task["task_id"])
                if manifest_task_id in adopted:
                    path, result = adopted[manifest_task_id]
                    records.append(
                        record_for(
                            task=manifest_task,
                            pair_cap=adopted_caps.get(manifest_task_id),
                            result_path=path,
                            result=result,
                            lifecycle="isolated_vm",
                            attestation_errors=quarantines.get(
                                manifest_task_id
                            ),
                            posthoc_revalidation=revalidations.get(
                                manifest_task_id
                            ),
                        )
                    )
                elif manifest_task_id in new_results:
                    path, result, cap = new_results[manifest_task_id]
                    records.append(
                        record_for(
                            task=manifest_task,
                            pair_cap=cap,
                            result_path=path,
                            result=result,
                            lifecycle=persistent_lifecycle["mode"],
                            attestation_errors=quarantines.get(
                                manifest_task_id
                            ),
                            posthoc_revalidation=revalidations.get(
                                manifest_task_id
                            ),
                        )
                    )
            write_summary(
                args=args,
                manifest=manifest,
                tasks=tasks,
                records=records,
                infrastructure_attempts=infrastructure_attempts,
                persistent_lifecycle=persistent_lifecycle,
                stopped_reason=stopped_reason,
                final=False,
            )
            if (
                args.max_new_tasks_per_vm is not None
                and len(new_results) >= args.max_new_tasks_per_vm
                and len(adopted) + len(new_results) < len(tasks)
            ):
                stopped_reason = "per-VM task limit reached"
                break
    except Exception as exc:
        lifecycle_error = f"{type(exc).__name__}: {exc}"
        stopped_reason = lifecycle_error
    finally:
        try:
            paired.stop_pilot(process)
        except Exception as exc:
            cleanup_error = f"{type(exc).__name__}: {exc}"
            lifecycle_error = (
                f"{lifecycle_error}; cleanup stop failed: {cleanup_error}"
                if lifecycle_error
                else f"cleanup stop failed: {cleanup_error}"
            )
        finally:
            log_stream.close()
        try:
            if live is not None:
                cleanup = paired.pilot_cleanup_record(live)
            else:
                cleanup = paired.pilot_cleanup_record_from_log(fleet_log_path)
        except Exception as exc:
            cleanup_error = f"{type(exc).__name__}: {exc}"
            lifecycle_error = (
                f"{lifecycle_error}; cleanup verification failed: {cleanup_error}"
                if lifecycle_error
                else f"cleanup verification failed: {cleanup_error}"
            )

    persistent_lifecycle["cleanup"] = cleanup
    persistent_lifecycle["cleanup_verified"] = bool(
        cleanup
        and (cleanup.get("record") or {}).get("cleanup_verified") is True
    )
    persistent_lifecycle["error"] = lifecycle_error

    records = []
    for task in tasks:
        task_id = str(task["task_id"])
        if task_id in adopted:
            path, result = adopted[task_id]
            records.append(
                record_for(
                    task=task,
                    pair_cap=adopted_caps.get(task_id),
                    result_path=path,
                    result=result,
                    lifecycle="isolated_vm",
                    attestation_errors=quarantines.get(task_id),
                    posthoc_revalidation=revalidations.get(task_id),
                )
            )
        elif task_id in new_results:
            path, result, cap = new_results[task_id]
            result["fleet_cleanup"] = cleanup
            paired.write_json(path, result)
            records.append(
                record_for(
                    task=task,
                    pair_cap=cap,
                    result_path=path,
                    result=result,
                    lifecycle=persistent_lifecycle["mode"],
                    attestation_errors=quarantines.get(task_id),
                    posthoc_revalidation=revalidations.get(task_id),
                )
            )

    if lifecycle_error and stopped_reason is None:
        stopped_reason = lifecycle_error
    summary = write_summary(
        args=args,
        manifest=manifest,
        tasks=tasks,
        records=records,
        infrastructure_attempts=infrastructure_attempts,
        persistent_lifecycle=persistent_lifecycle,
        stopped_reason=stopped_reason,
        final=True,
    )
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PersistentCertificationError as exc:
        print(json.dumps({"status": "error", "message": str(exc)}))
        raise SystemExit(1)
