#!/usr/bin/env python3
"""Finalize a last-arm session cleanup using verified Fleet VM deletion."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import run_paired_gpt55 as paired
import run_persistent_web_certification as persistent


class FinalizationError(RuntimeError):
    pass


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise FinalizationError(f"{path} must contain an object")
    return value


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--partial-paired-result", type=Path, required=True)
    parser.add_argument("--provenance", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    partial = read_json(args.partial_paired_result)
    provenance = read_json(args.provenance)
    errors = partial.get("pair_validation_errors") or []
    if partial.get("pair_valid") is not False or len(errors) != 1:
        raise FinalizationError("input must have exactly one validation error")
    expected_error = "Cua Driver session cleanup was not verified"
    if expected_error not in str(errors[0]):
        raise FinalizationError("validation error is not a session cleanup failure")
    if partial.get("run_error") is not None:
        raise FinalizationError("paired run has an unrelated run error")

    episodes = partial.get("episodes") or []
    if len(episodes) != 2:
        raise FinalizationError("paired result must contain both arms")
    failed = [
        episode
        for episode in episodes
        if episode.get("session_ended") is not True
    ]
    if len(failed) != 1 or failed[0] is not episodes[-1]:
        raise FinalizationError("only the last arm may have unverified cleanup")
    for episode in episodes:
        if (
            episode.get("agent_failure") is not None
            or not isinstance(episode.get("evaluation"), dict)
        ):
            raise FinalizationError("an arm has an unrelated execution failure")

    cleanup = partial.get("fleet_cleanup") or {}
    cleanup_path = Path(str(cleanup.get("path") or ""))
    cleanup_record = cleanup.get("record") or {}
    if (
        not cleanup_path.is_file()
        or cleanup_record.get("cleanup_verified") is not True
    ):
        raise FinalizationError("Fleet cleanup is not verified")
    on_disk_cleanup = read_json(cleanup_path)
    if on_disk_cleanup != cleanup_record:
        raise FinalizationError("embedded Fleet cleanup differs from its record")
    fleet = provenance.get("fleet") or {}
    if (
        not cleanup_record.get("namespace")
        or cleanup_record.get("namespace") != fleet.get("namespace")
    ):
        raise FinalizationError("cleanup namespace does not match provenance")

    finalized_episodes = []
    for episode in episodes:
        finalized_episode = dict(episode)
        if episode is failed[0]:
            finalized_episode["session_ended"] = True
            finalized_episode["session_end_evidence"] = {
                "method": "verified_fleet_vm_deletion",
                "explanation": (
                    "The end_session RPC did not return a verified response. "
                    "The arm was last in the pair, and Fleet subsequently "
                    "verified deletion of its claim, pool, VM namespace, and "
                    "in-memory Cua Driver process. No session remained live."
                ),
                "cleanup_record_path": str(cleanup_path.resolve()),
                "cleanup_record_sha256": paired.sha256_file(cleanup_path),
            }
        finalized_episodes.append(finalized_episode)

    finalized = persistent.build_paired_result(
        task_id=str(partial.get("task_id")),
        requested_model=str(partial.get("requested_model")),
        order=str(partial.get("order")),
        results=finalized_episodes,
        cleanup=cleanup,
        run_error=None,
    )
    finalized["supersedes_result"] = {
        "path": str(args.partial_paired_result.resolve()),
        "sha256": paired.sha256_file(args.partial_paired_result),
        "reason": (
            "The last arm's end_session RPC was unverified. Fleet deletion "
            "provides stronger terminal evidence that the VM-local session "
            "and Driver process no longer existed."
        ),
    }
    finalized["posthoc_session_cleanup_finalization"] = {
        "schema_version": 1,
        "provenance_path": str(args.provenance.resolve()),
        "provenance_sha256": paired.sha256_file(args.provenance),
        "cleanup_record_path": str(cleanup_path.resolve()),
        "cleanup_record_sha256": paired.sha256_file(cleanup_path),
        "terminal_arm": failed[0].get("mode"),
    }
    if args.output_dir.exists():
        raise FinalizationError("--output-dir must not already exist")
    args.output_dir.mkdir(parents=True)
    paired.write_json(args.output_dir / "paired-result.json", finalized)
    print(json.dumps(finalized, indent=2, sort_keys=True))
    return 0 if finalized["pair_valid"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except FinalizationError as exc:
        print(json.dumps({"status": "error", "message": str(exc)}))
        raise SystemExit(1)
