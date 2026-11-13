from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL_LOCAL = REPO_ROOT / "libs/cua-driver/scripts/uninstall-local.sh"
SCRIPTS = REPO_ROOT / "libs/cua-driver/scripts"


def _executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def test_unix_local_uninstall_removes_owned_links_and_preserves_release(tmp_path: Path) -> None:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    local_home = home / ".cua-driver-local"
    release_home = home / ".cua-driver"
    local_bin = home / ".local/bin"
    local_cache = home / ".cache/cua-driver-local"
    release_cache = home / ".cache/cua-driver"
    for path in (fake_bin, local_home, release_home, local_bin, local_cache, release_cache):
        path.mkdir(parents=True, exist_ok=True)

    local_marker = local_home / "packages/current/cua-driver-local"
    local_marker.parent.mkdir(parents=True)
    local_marker.write_text("local\n", encoding="utf-8")
    for runtime_file in (
        ".telemetry_identity.lock",
        ".telemetry_lifecycle.lock",
        ".telemetry_retry_after",
    ):
        (local_home / runtime_file).write_text("local runtime state\n", encoding="utf-8")
    release_marker = local_home / ".release_installed/0.23.1"
    release_marker.parent.mkdir()
    release_marker.write_text("", encoding="utf-8")

    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pkill", "exit 0")
    _executable(fake_bin / "systemctl", "exit 0")

    local_cli = local_bin / "cua-driver-local"
    local_cli.symlink_to(local_home / "packages/current/cua-driver-local")
    release_cli = local_bin / "cua-driver"
    release_cli.symlink_to(release_home / "packages/current/cua-driver")

    local_skill = home / ".agents/skills/cua-driver"
    local_skill.parent.mkdir(parents=True)
    local_skill.symlink_to(local_home / "skills/cua-driver")

    local_unit = home / ".config/systemd/user/cua-driver-local.service"
    release_unit = home / ".config/systemd/user/cua-driver.service"
    local_unit.parent.mkdir(parents=True)
    local_unit.write_text("local\n", encoding="utf-8")
    release_unit.write_text("release\n", encoding="utf-8")

    claude_json = home / ".claude.json"
    claude_json.write_text(
        json.dumps(
            {
                "mcpServers": {
                    "local": {"command": str(local_cli), "args": ["mcp"]},
                    "release": {"command": str(release_cli), "args": ["mcp"]},
                }
            }
        ),
        encoding="utf-8",
    )

    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_LOCAL_HOME": str(local_home),
            "CUA_DRIVER_LOCAL_INSTALL_DIR": str(local_bin),
        }
    )
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--force", "--keep-tcc"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr

    assert not local_cli.exists() and not local_cli.is_symlink()
    assert not local_skill.exists() and not local_skill.is_symlink()
    assert not local_home.exists()
    assert not local_cache.exists()
    assert not local_unit.exists()

    assert release_cli.is_symlink()
    assert release_home.exists()
    assert release_cache.exists()
    assert release_unit.exists()
    remaining = json.loads(claude_json.read_text(encoding="utf-8"))["mcpServers"]
    assert set(remaining) == {"release"}


def test_unix_local_uninstall_keeps_shared_skill_link_owned_by_release(tmp_path: Path) -> None:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    release_skill_target = home / ".cua-driver/skills/cua-driver"
    release_skill_target.mkdir(parents=True)
    skill_link = home / ".agents/skills/cua-driver"
    skill_link.parent.mkdir(parents=True)
    skill_link.symlink_to(release_skill_target)
    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    _executable(fake_bin / "pkill", "exit 0")

    env = os.environ.copy()
    env.update({"HOME": str(home), "PATH": f"{fake_bin}:/usr/bin:/bin"})
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--force", "--keep-tcc"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert skill_link.is_symlink()
    assert skill_link.resolve() == release_skill_target


def test_local_uninstall_contract_is_explicit_on_both_platforms() -> None:
    unix = (SCRIPTS / "uninstall-local.sh").read_text()
    windows = (SCRIPTS / "uninstall-local.ps1").read_text(encoding="utf-8-sig")

    for token in (
        "/Applications/CuaDriverLocal.app",
        "com.trycua.driver.local",
        ".cua-driver-local",
        "cua-driver-local.service",
        "com.trycua.cua-driver-local.plist",
    ):
        assert token in unix
    for token in (
        "cua-driver-local-serve",
        "cua-driver-uia-local",
        ".cua-driver-local",
        "Programs\\Cua\\cua-driver-local\\bin",
    ):
        assert token in windows
    assert "Test-LocalLinkTarget" in windows
    assert "is_local_target" in unix
    assert "LOCAL_HOME_MARKER" in unix
    assert "$LocalHomeMarker" in windows
    assert "Remove-Item -LiteralPath $HomeDir -Force -Recurse" not in windows
    assert 'rm -rf "$HOME_DIR"' not in unix
    assert "--validate-only" in unix
    assert "$ValidateOnly" in windows
    for token in (
        ".telemetry_identity.lock",
        ".telemetry_lifecycle.lock",
        ".telemetry_retry_after",
    ):
        assert token in unix
        assert token in windows


def test_unix_local_uninstall_rejects_release_home_override(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()
    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "CUA_DRIVER_LOCAL_HOME": str(home / ".cua-driver"),
        }
    )
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--validate-only"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 2
    assert "release-owned local home" in result.stderr


def test_unix_release_uninstaller_declares_local_detection_inputs() -> None:
    unix = (SCRIPTS / "uninstall.sh").read_text(encoding="utf-8-sig")

    for token in (
        "CUA_DRIVER_LOCAL_HOME",
        "CUA_DRIVER_LOCAL_INSTALL_DIR",
        "/Applications/CuaDriverLocal.app",
    ):
        assert token in unix


def _darwin_local_sandbox(tmp_path: Path) -> tuple[Path, dict[str, str]]:
    """A fake macOS where nothing outside the sandbox HOME can be removed.

    uninstall-local.sh deletes /Applications/CuaDriverLocal.app on Darwin, so
    the `rm` and `sudo` shims make that a recorded no-op on a developer Mac.
    """
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    calls = tmp_path / "calls.log"
    home.mkdir()
    for command in ("dirname", "readlink", "realpath", "rmdir", "sed"):
        executable = shutil.which(command, path=os.defpath)
        assert executable is not None, f"missing fixture utility: {command}"
        (fake_bin / command).parent.mkdir(parents=True, exist_ok=True)
        (fake_bin / command).symlink_to(executable)
    _executable(fake_bin / "uname", "printf 'Darwin\\n'")
    _executable(fake_bin / "pgrep", "exit 1\n")
    for command in ("pkill", "launchctl", "tccutil", "sudo", "rm"):
        _executable(fake_bin / command, f"printf '%s:%s\\n' '{command}' \"$*\" >> '{calls}'\n")
    env = {
        "HOME": str(home),
        "PATH": str(fake_bin),
        "CUA_DRIVER_LOCAL_HOME": str(home / ".cua-driver-local"),
        "CUA_DRIVER_LOCAL_INSTALL_DIR": str(home / ".local/bin"),
    }
    return calls, env


def _run_local_uninstall(env: dict[str, str], *flags: str) -> None:
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--force", *flags],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_local_uninstall_keeps_macos_permissions_by_default(tmp_path: Path) -> None:
    calls, env = _darwin_local_sandbox(tmp_path)

    _run_local_uninstall(env)

    recorded = calls.read_text(encoding="utf-8") if calls.exists() else ""
    assert "tccutil:" not in recorded


def test_local_uninstall_resets_macos_permissions_only_when_asked(tmp_path: Path) -> None:
    calls, env = _darwin_local_sandbox(tmp_path)

    _run_local_uninstall(env, "--reset-permissions")

    lines = [line for line in calls.read_text(encoding="utf-8").splitlines() if line.startswith("tccutil:")]
    assert lines == [
        "tccutil:reset Accessibility com.trycua.driver.local",
        "tccutil:reset ScreenCapture com.trycua.driver.local",
        "tccutil:reset AppleEvents com.trycua.driver.local",
    ]


def test_local_uninstall_stops_daemon_started_from_macos_app_bundle(tmp_path: Path) -> None:
    calls, env = _darwin_local_sandbox(tmp_path)

    _run_local_uninstall(env)

    app_daemon = r"/Applications/CuaDriverLocal\.app/Contents/MacOS/cua-driver-local"
    lines = calls.read_text(encoding="utf-8").splitlines()
    assert f"pkill:-f ^{app_daemon}([[:space:]]|$)" in lines


@pytest.mark.parametrize("pgrep_status", [0, 2], ids=["daemon-alive", "pgrep-error"])
def test_local_uninstall_fails_while_a_local_daemon_survives(tmp_path: Path, pgrep_status: int) -> None:
    calls, env = _darwin_local_sandbox(tmp_path)
    fake_bin = Path(env["PATH"])
    _executable(fake_bin / "pgrep", f"exit {pgrep_status}\n")
    _executable(fake_bin / "sleep", "exit 0\n")
    cache = Path(env["HOME"]) / "Library/Caches/cua-driver-local"
    cache.mkdir(parents=True)
    marker = Path(env["CUA_DRIVER_LOCAL_HOME"]) / "packages/current/cua-driver-local"
    marker.parent.mkdir(parents=True)
    marker.write_text("local\n", encoding="utf-8")
    launchagent = Path(env["HOME"]) / "Library/LaunchAgents/com.trycua.cua-driver-local.plist"
    launchagent.parent.mkdir(parents=True)
    launchagent.write_text("local\n", encoding="utf-8")

    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL_LOCAL), "--force"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode != 0
    assert "still running" in result.stderr
    assert "uninstalled" not in result.stdout
    recorded = calls.read_text(encoding="utf-8")
    assert "pkill:-KILL -f" in recorded
    assert "rm:" not in recorded
    assert cache.is_dir()
    assert marker.is_file()
    assert launchagent.is_file()


def test_unix_local_uninstall_stops_a_running_local_daemon(tmp_path: Path) -> None:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    # Regex metacharacters in an overridden home must still match literally.
    local_home = home / "local.home (x)"
    daemon = local_home / "packages/current/cua-driver-local"
    _executable(daemon, "exec sleep 60\n")
    _executable(fake_bin / "uname", "printf 'Linux\\n'")
    # argv[0] is the daemon path, as for a real `cua-driver-local serve`.
    process = subprocess.Popen(["/bin/bash", "-c", f"exec -a '{daemon}' sleep 60"])
    try:
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{fake_bin}:/usr/bin:/bin",
                "CUA_DRIVER_LOCAL_HOME": str(local_home),
                "CUA_DRIVER_LOCAL_INSTALL_DIR": str(home / ".local/bin"),
            }
        )
        result = subprocess.run(
            ["/bin/bash", str(UNINSTALL_LOCAL), "--force", "--keep-tcc"],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
            timeout=30,
        )
        process.wait(timeout=10)
    finally:
        if process.poll() is None:
            process.kill()
    assert result.returncode == 0, result.stdout + result.stderr
    assert not daemon.exists()
