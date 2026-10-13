"""Filesystem integration of production cleanup receipts and offline verifier.

Resource probes are stubbed; these tests do NOT constitute Docker/QEMU E2E.
"""
import json

import pytest

from cua_bench.runner import task_runner as module
from cua_bench.verify_execution import verify_execution


def _write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value) + "\n", encoding="utf-8")


def _other_receipts(root, task_id):
    _write_json(root / "oracle-result.json", {
        "schema_version": "cua-bench-oracle-result/v1",
        "execution_id": task_id, "task_index": 0, "evaluated": True,
        "success": True, "rewards": [1.0], "success_threshold": 1.0,
    })
    _write_json(root / "task_0_agent_logs" / "executed-actions.jsonl", {
        "schema_version": "cua-bench-executed-action/v1",
        "execution_id": task_id, "sequence": 1,
        "execution_status": "returned", "action": {"type": "click"},
        "before": {"sha256": "a" * 64, "size_bytes": 1},
        "after": {"sha256": "b" * 64, "size_bytes": 1},
    })


@pytest.mark.asyncio
@pytest.mark.parametrize("residual", [False, True])
async def test_production_cleanup_receipt_consumed_by_offline_verifier(
    tmp_path, monkeypatch, residual,
):
    task_id = "integration-1"
    runner = module.TaskRunner()
    monkeypatch.setattr(module, "get_overlays_path", lambda: tmp_path / "overlays")

    async def inspect(kind, name):
        return not (residual and name == f"cua-agent-{task_id}")

    monkeypatch.setattr(runner, "_resource_absent", inspect)
    _other_receipts(tmp_path, task_id)
    await runner._write_cleanup_receipt(task_id, str(tmp_path))

    receipt = json.loads((tmp_path / "cleanup-result.json").read_text())
    assert set(receipt["resources"]) == {
        "agent_container", "env_container", "network", "overlay",
    }
    assert receipt["verified"] is not residual

    report = verify_execution(tmp_path, 0, task_id)
    assert report["checks"]["oracle"] is True
    assert report["checks"]["actions"] is True
    assert report["checks"]["cleanup"] is not residual
    assert report["run_accepted"] is not residual
