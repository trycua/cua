"""Shared contract validation: independent of Docker and Agent SDK."""
from cua_bench.evidence_contract import action_event_valid, oracle_receipt_valid


def oracle():
    return dict(schema_version="cua-bench-oracle-result/v1", task_index=0,
                execution_id="task-1", evaluated=True, rewards=[1.0],
                success_threshold=1.0, success=True)


def action():
    obs = {"sha256": "a" * 64, "size_bytes": 1}
    return dict(schema_version="cua-bench-executed-action/v1",
                execution_id="task-1", sequence=1, execution_status="returned",
                action={"type": "click"}, before=obs, after=obs)


def test_valid_shared_oracle():
    assert oracle_receipt_valid(oracle(), 0, "task-1")


def test_reject_boolean_sequence():
    event = action()
    event["sequence"] = True
    assert not action_event_valid(event, 1, "task-1")


def test_reject_missing_observation_hash():
    event = action()
    event["before"] = {"error_type": "TimeoutError"}
    assert not action_event_valid(event, 1, "task-1")


def test_reject_forged_oracle_verdict():
    receipt = oracle()
    receipt["success"] = False
    assert not oracle_receipt_valid(receipt, 0, "task-1")


def test_finite_reward_above_one_is_valid_per_solver():
    receipt = oracle()
    receipt["rewards"] = [2.0]
    assert oracle_receipt_valid(receipt, 0, "task-1")


def test_failed_event_is_valid_evidence_but_not_success():
    event = action()
    event["execution_status"] = "failed"
    assert action_event_valid(event, 1, "task-1")
