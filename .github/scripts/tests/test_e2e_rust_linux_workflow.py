from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/e2e-rust-linux.yml"


def test_canonical_certification_is_fail_closed_and_complete() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    certification = workflow.split("\n  certify:\n", 1)[1]

    # Manual dispatches and stable-tag release-gate calls both certify.
    assert "if: success() && inputs.lane == 'all'" in certification
    assert "needs: [source, shared, native, capture, installer]" in certification
    assert 'schema "cua-driver/e2e-certification/v1"' in certification
    assert 'platform "linux"' in certification
    for job in ("shared", "native", "capture", "installer"):
        assert f'{job}: "success"' in certification
    assert "name: rust-linux-e2e-certification" in certification


def test_release_gate_call_matches_dispatch_and_ignores_caller_artifacts() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    trigger = workflow.split("\npermissions:", 1)[0]
    call, dispatch = trigger.split("  workflow_call:\n", 1)[1].split("  workflow_dispatch:\n", 1)
    for name in ("ref:", "lane:", "cell_filter:"):
        assert name in call
        assert name in dispatch
    assert "github.event.inputs" not in workflow
    assert "pattern: rust-linux-*" in workflow
