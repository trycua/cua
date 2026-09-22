from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/e2e-rust-linux.yml"


def test_canonical_certification_is_fail_closed_and_complete() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    certification = workflow.split("\n  certify:\n", 1)[1]

    assert (
        "if: success() && github.event_name == 'workflow_dispatch' && inputs.lane == 'all'"
        in certification
    )
    assert "needs: [source, shared, native, capture, installer]" in certification
    assert 'schema "cua-driver/e2e-certification/v1"' in certification
    assert 'platform "linux"' in certification
    for job in ("shared", "native", "capture", "installer"):
        assert f'{job}: "success"' in certification
    assert "name: rust-linux-e2e-certification" in certification
