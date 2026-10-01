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


def test_native_appkit_job_uses_released_driver_and_source_harness() -> None:
    """RFC #4268 Phase 1: native tasks on the AppKit harness with the mock provider."""
    jobs = load()["jobs"]
    assert jobs["native-appkit-macos"]["runs-on"].startswith("macos-")
    native = job_text("native-appkit-macos")
    assert "https://cua.ai/driver/install.sh" in native
    assert "cargo build" not in native
    assert 'MIN_RELEASED_DRIVER_VERSION: "0.30.1"' in native
    assert "tests/fixtures/build/macos.sh --only appkit" in native
    assert "--app /Applications/CuaDriver.app --expected-client com.trycua.driver" in native
    assert "verify_native.py --typescript" in native
    assert "--live" not in native
    # The audit reads the harness-owned oracle result and the v2 contract.
    assert 'check["verified"] is True' in native
    assert "cua.jev_choice_request_v2" in native


def test_native_harness_changes_trigger_the_workflow() -> None:
    triggers = load()[True]
    for event in ("pull_request", "push"):
        assert "libs/cua-driver/tests/fixtures/apps/macos/appkit/**" in triggers[event]["paths"]


def test_native_wpf_job_uses_released_driver_default_install_and_source_harness() -> None:
    """RFC #4268 Phase 2: native tasks on the WPF harness (Windows UIA)."""
    jobs = load()["jobs"]
    assert jobs["native-wpf-windows"]["runs-on"].startswith("windows-")
    native = job_text("native-wpf-windows")
    assert "https://cua.ai/driver/install.ps1" in native
    assert "cargo build" not in native
    assert 'MIN_RELEASED_DRIVER_VERSION: "0.30.1"' in native
    # The default install registers the elevated autostart; no opt-out flag.
    assert '$task.Principal.RunLevel -ne "Highest"' in native
    assert "-NoAutostart" not in native
    assert "verify-user-session.ps1" in native
    assert "tests\\fixtures\\build\\windows.ps1 -Targets wpf" in native
    assert "verify_native.py --harness wpf --typescript" in native
    assert "--live" not in native
    assert 'check["verified"] is True' in native
    assert "cua.jev_choice_request_v2" in native


def test_native_winui3_job_uses_released_driver_default_install_and_source_harness() -> None:
    """RFC #4268 (#4314): native tasks on the WinUI3 harness (Windows UIA)."""
    jobs = load()["jobs"]
    assert jobs["native-winui3-windows"]["runs-on"].startswith("windows-")
    native = job_text("native-winui3-windows")
    assert "https://cua.ai/driver/install.ps1" in native
    assert "cargo build" not in native
    assert 'MIN_RELEASED_DRIVER_VERSION: "0.30.1"' in native
    assert '$task.Principal.RunLevel -ne "Highest"' in native
    assert "-NoAutostart" not in native
    assert "verify-user-session.ps1" in native
    assert "tests\\fixtures\\build\\windows.ps1 -Targets winui3" in native
    assert "verify_native.py --harness winui3 --typescript" in native
    assert "--live" not in native
    assert 'check["verified"] is True' in native
    assert "cua.jev_choice_request_v2" in native
    triggers = load()[True]
    for event in ("pull_request", "push"):
        assert "libs/cua-driver/tests/fixtures/apps/windows/winui3/**" in triggers[event]["paths"]


def test_native_windows_jobs_run_distractor_density_24() -> None:
    """#4312: the WPF and WinUI3 jobs also verify the tasks past the 24-action cap."""
    triggers = load()[True]
    for harness in ("wpf", "winui3"):
        native = job_text(f"native-{harness}-windows")
        assert f"verify_native.py --harness {harness} --density 24" in native
        assert 'summary["density"] == 24' in native
        assert 'check["distractor_actions"] == 0' in native
        assert 'check["max_candidates"] == 26' in native
        assert "jev-use-native-density-window-state/*.json" in native
        for event in ("pull_request", "push"):
            assert f"libs/cua-driver/tests/fixtures/apps/windows/{harness}/**" in triggers[event]["paths"]


def test_native_gtk3_job_uses_released_driver_and_x11_stack() -> None:
    """RFC #4268 Phase 2: native tasks on the GTK3 harness (Linux AT-SPI)."""
    jobs = load()["jobs"]
    assert jobs["native-gtk3-linux"]["runs-on"].startswith("ubuntu-")
    native = job_text("native-gtk3-linux")
    assert "https://cua.ai/driver/install.sh" in native
    assert "cargo build" not in native
    assert 'MIN_RELEASED_DRIVER_VERSION: "0.30.1"' in native
    assert "tests/fixtures/build/linux.sh --only gtk3" in native
    for piece in ("xvfb-run", "dbus-run-session", "openbox", "at-spi2-core", "python3-gi"):
        assert piece in native
    assert "verify_native.py --harness gtk3 --typescript" in native
    # save-note is gated on the first Driver that reports named AT-SPI text
    # values (#4291), and the audit still requires the other two tasks.
    assert 'SAVE_NOTE_MIN_DRIVER_VERSION: "0.30.3"' in native
    assert 'assert {"gtk3-counter", "gtk3-choose-size"} <= tasks' in native
    assert "--live" not in native
    assert 'check["verified"] is True' in native
    assert "cua.jev_choice_request_v2" in native


def test_windows_and_linux_harness_changes_trigger_the_workflow() -> None:
    triggers = load()[True]
    for event in ("pull_request", "push"):
        paths = triggers[event]["paths"]
        for path in (
            "libs/cua-driver/tests/fixtures/apps/windows/wpf/**",
            "libs/cua-driver/tests/fixtures/build/windows.ps1",
            "libs/cua-driver/tests/fixtures/apps/linux/gtk3/**",
            "libs/cua-driver/tests/fixtures/build/linux.sh",
        ):
            assert path in paths
