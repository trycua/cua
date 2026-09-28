"""Keep the jev-use released-Driver proof on macOS and Windows secret-free and user-shaped."""

from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/ci-jev-use.yml"
WINDOWS_PROOF_SCRIPT = "scripts/ci/windows/run-jev-use-elevated-autostart.ps1"
MIN_RELEASED_DRIVER_VERSION = (0, 30, 1)


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
    assert "cargo build" not in windows


def test_released_driver_jobs_use_default_version_resolution_with_a_floor() -> None:
    jobs = load()["jobs"]
    for name in ("released-driver-macos", "released-driver-windows"):
        text = job_text(name)
        # No pin: the canonical installer's own resolution picks the release.
        assert "CUA_DRIVER_RS_VERSION" not in text, name
        assert "CUA_DRIVER_VERSION" not in text, name
        assert "-Release" not in text, name
        floor = jobs[name]["env"]["MIN_RELEASED_DRIVER_VERSION"]
        assert tuple(int(part) for part in floor.split(".")) >= MIN_RELEASED_DRIVER_VERSION, name
        assert "older than" in text, name
    assert "sort -V" in job_text("released-driver-macos")
    assert "[version]$env:MIN_RELEASED_DRIVER_VERSION" in job_text("released-driver-windows")


def test_windows_proves_the_default_elevated_autostart_path() -> None:
    windows = job_text("released-driver-windows")
    assert "-NoAutoStart" not in windows
    assert "-AutoStart:$false" not in windows
    assert "invoke-standard-user-token.ps1" not in windows
    assert "run-jev-use-standard-user.ps1" not in windows
    assert not (ROOT / "scripts/ci/windows/invoke-standard-user-token.ps1").exists()
    assert "verify-user-session.ps1" in windows
    assert WINDOWS_PROOF_SCRIPT.replace("/", "\\") in windows
    script = (ROOT / WINDOWS_PROOF_SCRIPT).read_text(encoding="utf-8")
    # The default task, started the way the installer tells users to.
    assert 'Get-ScheduledTask -TaskName "cua-driver-serve"' in script
    assert '-ne "Highest"' in script
    assert "autostart kick" in script
    assert "Start-Process" not in script
    # Independent token oracle: elevated daemon, de-elevated browsers it launched.
    assert "TokenIntegrityLevel" in script
    assert "S-1-5-32-544" in script
    assert "the autostart daemon must be elevated" in script
    assert "ran with a privileged token" in script
    assert "was not launched by a readable cua-driver.exe" in script
    assert "does not prove the elevated path" in script
    assert "is not below its launching Driver" in script


def test_released_driver_jobs_run_both_languages_and_audit_the_state_oracle() -> None:
    for name in ("released-driver-macos", "released-driver-windows"):
        text = job_text(name)
        assert '"observed"] == {"submitted": "jev-guide-mock"}' in text, name
        assert '("typescript", "mock", "verified")' in text, name
    assert "verify_setup.py --typescript" in job_text("released-driver-macos")
    inner = (ROOT / WINDOWS_PROOF_SCRIPT).read_text(encoding="utf-8")
    assert "verify_setup.py --typescript" in inner


def test_windows_proof_script_triggers_the_workflow() -> None:
    triggers = load()[True]
    for event in ("pull_request", "push"):
        assert WINDOWS_PROOF_SCRIPT in triggers[event]["paths"], event
        assert not any("standard-user" in path for path in triggers[event]["paths"]), event


def test_macos_seeds_tcc_only_for_the_released_app_identity() -> None:
    macos = job_text("released-driver-macos")
    assert "seed-tcc-guest.sh" in macos
    assert "--app /Applications/CuaDriver.app --expected-client com.trycua.driver\n" in macos
    assert "install-local.sh" not in macos
