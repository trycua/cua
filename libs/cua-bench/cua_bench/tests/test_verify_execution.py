"""Offline verifier contract tests. No Docker or model credentials required."""
import json

from cua_bench.verify_execution import verify_execution


def write_json(path, obj):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj) + "\n", encoding="utf-8")


def make_receipts(root, execution_id="run-1"):
    write_json(root / "oracle-result.json", {
        "schema_version": "cua-bench-oracle-result/v1",
        "execution_id": execution_id,
        "task_index": 0,
        "evaluated": True, "success": True,
        "rewards": [1.0], "success_threshold": 1.0,
    })
    write_json(root / "cleanup-result.json", {
        "schema_version": "cua-bench-cleanup/v1",
        "task_id": execution_id,
        "verified": True,
        "resources": {"agent_container": {"name": "a", "absent": True}},
    })
    logs = root / "task_0_agent_logs" / "executed-actions.jsonl"
    write_json(logs, {
        "schema_version": "cua-bench-executed-action/v1",
        "execution_id": execution_id, "sequence": 1,
        "execution_status": "returned", "action": {"type": "click"},
        "before": {"sha256": "a" * 64, "size_bytes": 1},
        "after": {"sha256": "b" * 64, "size_bytes": 1},
    })


def test_complete_receipts_pass_internal_consistency(tmp_path):
    make_receipts(tmp_path)
    result = verify_execution(tmp_path, 0, "run-1")
    assert result["valid"] is True
    assert result["verified_action_count"] == 1


def test_wrong_execution_id_fails_closed(tmp_path):
    make_receipts(tmp_path)
    result = verify_execution(tmp_path, 0, "run-2")
    assert result["valid"] is False
    assert not result["checks"]["oracle"]


def test_missing_action_evidence_fails_closed(tmp_path):
    make_receipts(tmp_path)
    (tmp_path / "task_0_agent_logs" / "executed-actions.jsonl").unlink()
    assert verify_execution(tmp_path, 0, "run-1")["valid"] is False


def test_forged_oracle_success_is_detected(tmp_path):
    make_receipts(tmp_path)
    p = tmp_path / "oracle-result.json"
    value = json.loads(p.read_text())
    value["rewards"] = [0.0]
    write_json(p, value)
    assert verify_execution(tmp_path, 0, "run-1")["checks"]["oracle"] is False


def test_cleanup_residual_is_rejected(tmp_path):
    make_receipts(tmp_path)
    p = tmp_path / "cleanup-result.json"
    value = json.loads(p.read_text())
    value["resources"]["agent_container"]["absent"] = False
    write_json(p, value)
    assert verify_execution(tmp_path, 0, "run-1")["checks"]["cleanup"] is False


def test_oracle_only_mode_allows_no_agent_stream(tmp_path):
    make_receipts(tmp_path)
    (tmp_path / "task_0_agent_logs" / "executed-actions.jsonl").unlink()
    result = verify_execution(tmp_path, 0, "run-1", require_actions=False)
    assert result["valid"] is True
    assert result["checks"]["actions"] is None


def test_finite_negative_reward_uses_explicit_threshold(tmp_path):
    make_receipts(tmp_path)
    p = tmp_path / "oracle-result.json"
    value = json.loads(p.read_text())
    value["rewards"] = [-0.25]
    value["success_threshold"] = 0.0
    value["success"] = False
    write_json(p, value)
    result = verify_execution(tmp_path, 0, "run-1")
    assert result["checks"]["oracle"] is True
    assert result["evidence_consistent"] is True
    assert result["task_success"] is False
    assert result["run_accepted"] is False


def test_failed_action_cannot_prove_successful_execution(tmp_path):
    make_receipts(tmp_path)
    p = tmp_path / "task_0_agent_logs" / "executed-actions.jsonl"
    value = json.loads(p.read_text())
    value["execution_status"] = "failed"
    write_json(p, value)
    result = verify_execution(tmp_path, 0, "run-1")
    assert result["valid"] is False
    assert result["checks"]["actions"] is True
    assert result["evidence_consistent"] is True
    assert result["failed_action_count"] == 1
    assert result["run_accepted"] is False


def test_missing_screenshot_observation_fails_closed(tmp_path):
    make_receipts(tmp_path)
    p = tmp_path / "task_0_agent_logs" / "executed-actions.jsonl"
    value = json.loads(p.read_text())
    value["before"] = {"error_type": "TimeoutError"}
    write_json(p, value)
    result = verify_execution(tmp_path, 0, "run-1")
    assert result["valid"] is False
    assert result["checks"]["actions"] is False


def test_failed_task_can_have_consistent_evidence(tmp_path):
    make_receipts(tmp_path)
    p = tmp_path / "oracle-result.json"
    value = json.loads(p.read_text())
    value["rewards"] = [0.0]
    value["success"] = False
    write_json(p, value)
    result = verify_execution(tmp_path, 0, "run-1")
    assert result["checks"]["oracle"] is True
    assert result["evidence_consistent"] is True
    assert result["task_success"] is False
    assert result["run_accepted"] is False
    assert result["valid"] is False
