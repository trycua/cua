"""Keep the jev-use released-Driver proof on macOS and Windows secret-free and user-shaped."""

from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/ci-jev-use.yml"
STANDARD_USER_SCRIPTS = (
    "scripts/ci/windows/invoke-standard-user-token.ps1",
    "scripts/ci/windows/run-jev-use-standard-user.ps1",
)


def load() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def job_text(name: str) -> str:
    """Return the raw YAML of one top-level job, so script bodies compare verbatim."""
    text = WORKFLOW.read_text(encoding="utf-8").split(f"\n  {name}:\n", 1)[1]
    lines = []
    for line in text.splitlines():
        if line and not line.startswith("    "):
            break
        lines.append(line)
    return "\n".join(lines)


def test_workflow_is_read_only_and_never_reads_secrets() -> None:
    workflow = load()
    assert workflow["permissions"] == {"contents": "read"}
    assert "secrets." not in WORKFLOW.read_text(encoding="utf-8")
    assert "pull_request_target" not in workflow[True]


def test_released_driver_jobs_run_on_hosted_macos_and_windows() -> None:
    jobs = load()["jobs"]
    assert jobs["released-driver-macos"]["runs-on"].startswith("macos-")
    assert jobs["released-driver-windows"]["runs-on"].startswith("windows-")


def test_released_driver_jobs_use_the_canonical_installers() -> None:
    macos = job_text("released-driver-macos")
    windows = job_text("released-driver-windows")
    assert "https://cua.ai/driver/install.sh" in macos
    assert "cargo build" not in macos
    assert "https://cua.ai/driver/install.ps1" in windows
    assert "-NoAutoStart" in windows
    assert "cargo build" not in windows


def test_released_driver_jobs_run_both_languages_and_audit_the_state_oracle() -> None:
    for name in ("released-driver-macos", "released-driver-windows"):
        text = job_text(name)
        assert '"observed"] == {"submitted": "jev-guide-mock"}' in text, name
        assert '("typescript", "mock", "verified")' in text, name
    assert "verify_setup.py --typescript" in job_text("released-driver-macos")
    inner = (ROOT / STANDARD_USER_SCRIPTS[1]).read_text(encoding="utf-8")
    assert "verify_setup.py --typescript" in inner


def test_windows_proof_runs_from_a_standard_user_token() -> None:
    windows = job_text("released-driver-windows")
    assert "verify-user-session.ps1" in windows
    assert "invoke-standard-user-token.ps1" in windows
    assert "run-jev-use-standard-user.ps1" in windows
    helper = (ROOT / STANDARD_USER_SCRIPTS[0]).read_text(encoding="utf-8")
    assert "SAFER_LEVELID_NORMALUSER = 0x20000" in helper
    inner = (ROOT / STANDARD_USER_SCRIPTS[1]).read_text(encoding="utf-8")
    assert "WindowsBuiltInRole]::Administrator" in inner
    assert "deny only" in inner


def test_helper_scripts_trigger_the_workflow() -> None:
    triggers = load()[True]
    for event in ("pull_request", "push"):
        for path in STANDARD_USER_SCRIPTS:
            assert path in triggers[event]["paths"], (event, path)
