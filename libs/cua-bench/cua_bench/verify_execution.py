"""Offline, read-only verifier for cua-bench execution artifacts.

Checks internal consistency, not the truth of screenshots or the remote oracle.
Usage: python -m cua_bench.verify_execution OUTPUT_DIR --task-index 0 --execution-id ID
"""
import argparse
import json
from .evidence_contract import action_event_valid, oracle_receipt_valid
from pathlib import Path


def _json(path):
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path.name}: expected object")
    return value


def verify_execution(root: Path, task_index: int, execution_id: str, *, require_actions=True):
    """Return a machine-readable fail-closed independent verification report."""
    errors = []
    checks = {}
    task_success = False
    failed_action_count = 0
    root = Path(root)
    oracle_path = root / "oracle-result.json"
    cleanup_path = root / "cleanup-result.json"
    action_path = root / f"task_{task_index}_agent_logs" / "executed-actions.jsonl"

    try:
        oracle = _json(oracle_path)
        rewards = oracle.get("rewards")
        threshold = oracle.get("success_threshold")
        valid = oracle_receipt_valid(oracle, task_index, execution_id)
        checks["oracle"] = bool(valid)
        task_success = bool(valid and oracle["success"])
        if not checks["oracle"]:
            errors.append("oracle_invalid")
    except (OSError, ValueError, TypeError, KeyError) as exc:
        checks["oracle"] = False
        errors.append(f"oracle_unreadable:{type(exc).__name__}")

    try:
        cleanup = _json(cleanup_path)
        resources = cleanup.get("resources")
        valid = (
            cleanup.get("schema_version") == "cua-bench-cleanup/v1"
            and cleanup.get("task_id") == execution_id
            and isinstance(resources, dict)
            and {"agent_container", "env_container", "network", "overlay"} <= resources.keys()
            and all(isinstance(v, dict) and v.get("absent") is True for v in resources.values())
            and cleanup.get("verified") is True
        )
        checks["cleanup"] = valid
        if not valid:
            errors.append("cleanup_unverified")
    except (OSError, ValueError, TypeError) as exc:
        checks["cleanup"] = False
        errors.append(f"cleanup_unreadable:{type(exc).__name__}")

    action_count = 0
    if require_actions:
        try:
            with action_path.open(encoding="utf-8") as stream:
                for line in stream:
                    record = json.loads(line)
                    action_count += 1
                    if not action_event_valid(record, action_count, execution_id):
                        raise ValueError("Invalid action event")
                    failed_action_count += record["execution_status"] == "failed"
            checks["actions"] = action_count > 0
            if not checks["actions"]:
                errors.append("actions_empty")
        except (OSError, ValueError, TypeError) as exc:
            checks["actions"] = False
            errors.append(f"actions_unreadable_or_invalid:{type(exc).__name__}")
    else:
        checks["actions"] = None

    evidence_consistent = not errors
    run_accepted = evidence_consistent and task_success and failed_action_count == 0
    return {
        "schema_version": "cua-bench-verification/v1",
        "execution_id": execution_id,
        "task_index": task_index,
        "valid": run_accepted,
        "evidence_consistent": evidence_consistent,
        "task_success": task_success,
        "run_accepted": run_accepted,
        "failed_action_count": failed_action_count,
        "checks": checks,
        "verified_action_count": action_count,
        "errors": errors,
        "limitations": [
            "Verifies internal artifact consistency only, not screenshot authenticity",
            "Does not independently re-execute or trustlessly validate the environment oracle",
            "Cannot establish that the recording intercepted all device actions",
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--task-index", required=True, type=int)
    parser.add_argument("--execution-id", required=True)
    parser.add_argument("--no-agent", action="store_true", help="Oracle-only task; no agent action stream")
    args = parser.parse_args()
    result = verify_execution(
        args.output_dir, args.task_index, args.execution_id,
        require_actions=not args.no_agent,
    )
    print(json.dumps(result, indent=2, sort_keys=True))
    raise SystemExit(0 if result["valid"] else 1)


if __name__ == "__main__":
    main()
