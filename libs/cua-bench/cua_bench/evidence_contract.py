"""Shared, dependency-free receipt contract for live and offline cua-bench verification.

This validates internally consistent claims, NOT the authenticity of the source.
"""
import math
import re


def valid_observation(value):
    return (
        isinstance(value, dict)
        and isinstance(value.get("sha256"), str)
        and re.fullmatch(r"[0-9a-f]{64}", value["sha256"]) is not None
        and type(value.get("size_bytes")) is int
        and value["size_bytes"] >= 0
    )


def oracle_receipt_valid(receipt, task_index, execution_id):
    if not isinstance(receipt, dict):
        return False
    rewards = receipt.get("rewards")
    threshold = receipt.get("success_threshold")
    if not (
        receipt.get("schema_version") == "cua-bench-oracle-result/v1"
        and receipt.get("task_index") == task_index
        and receipt.get("execution_id") == execution_id
        and receipt.get("evaluated") is True
        and type(receipt.get("success")) is bool
        and isinstance(rewards, list)
        and bool(rewards)
        and all(type(v) in (int, float) and math.isfinite(v) for v in rewards)
        and type(threshold) in (int, float)
        and math.isfinite(threshold)
        and 0 <= threshold <= 1
    ):
        return False
    return receipt["success"] == all(v >= threshold for v in rewards)


def action_event_valid(record, sequence, execution_id):
    return (
        isinstance(record, dict)
        and record.get("schema_version") == "cua-bench-executed-action/v1"
        and record.get("execution_id") == execution_id
        and type(record.get("sequence")) is int
        and record["sequence"] == sequence
        and record.get("execution_status") in ("returned", "failed")
        and isinstance(record.get("action"), dict)
        and valid_observation(record.get("before"))
        and valid_observation(record.get("after"))
    )
