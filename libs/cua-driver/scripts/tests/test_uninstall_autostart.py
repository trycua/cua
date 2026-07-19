from __future__ import annotations

import os
from pathlib import Path
import subprocess

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def _write_executable(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def _sandbox(tmp_path: Path, os_name: str) -> tuple[Path, Path, dict[str, str]]:
    home = tmp_path / "home"
    fake_bin = tmp_path / "bin"
    calls = tmp_path / "calls.log"
    home.mkdir()
    fake_bin.mkdir()

    _write_executable(fake_bin / "uname", f"printf '%s\\n' '{os_name}'\n")
    for command in ("launchctl", "systemctl", "tccutil", "sudo"):
        _write_executable(
            fake_bin / command,
            f"printf '%s:%s\\n' '{command}' \"$*\" >> '{calls}'\n",
        )

    # The uninstaller contains intentional recursive removal for real install
    # directories. Tests must never be able to reach those paths on a
    # developer machine, so the shim removes regular files/symlinks only when
    # they are below the sandbox HOME and records every outside request as a
    # no-op.
    _write_executable(
        fake_bin / "rm",
        f"""for argument in "$@"; do
    case "$argument" in
        -*) ;;
        '{home}'/*)
            if [ -d "$argument" ] && [ ! -L "$argument" ]; then
                printf 'rm-refused-directory:%s\\n' "$argument" >> '{calls}'
                exit 97
            fi
            /bin/rm -f -- "$argument"
            ;;
        *) printf 'rm-outside-noop:%s\\n' "$argument" >> '{calls}' ;;
    esac
done
""",
    )

    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
        }
    )
    env.pop("CUA_DRIVER_HOME", None)
    env.pop("CUA_DRIVER_RS_HOME", None)
    return home, calls, env


def _run_uninstall(env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        ["/bin/bash", str(UNINSTALL)],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return result


@pytest.mark.parametrize(
    ("os_name", "relative_paths", "expected_calls"),
    [
        (
            "Darwin",
            [
                "Library/LaunchAgents/com.trycua.cua-driver.plist",
                "Library/LaunchAgents/com.trycua.cua-driver-rs.plist",
            ],
            [
                "launchctl:unload {home}/Library/LaunchAgents/com.trycua.cua-driver.plist",
                "launchctl:unload {home}/Library/LaunchAgents/com.trycua.cua-driver-rs.plist",
            ],
        ),
        (
            "Linux",
            [
                ".config/systemd/user/cua-driver.service",
                ".config/systemd/user/cua-driver-rs.service",
            ],
            [
                "systemctl:--user stop cua-driver.service",
                "systemctl:--user disable cua-driver.service",
                "systemctl:--user stop cua-driver-rs.service",
                "systemctl:--user disable cua-driver-rs.service",
                "systemctl:--user daemon-reload",
            ],
        ),
    ],
)
def test_uninstall_removes_current_and_legacy_autostart(
    tmp_path: Path,
    os_name: str,
    relative_paths: list[str],
    expected_calls: list[str],
) -> None:
    home, calls, env = _sandbox(tmp_path, os_name)
    targets = [home / relative_path for relative_path in relative_paths]
    for target in targets:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("fixture\n", encoding="utf-8")

    _run_uninstall(env)

    assert all(not target.exists() for target in targets)
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    for expected_call in expected_calls:
        assert expected_call.format(home=home) in call_lines


@pytest.mark.parametrize(
    ("os_name", "current_autostart_path"),
    [
        ("Darwin", "Library/LaunchAgents/com.trycua.cua-driver.plist"),
        ("Linux", ".config/systemd/user/cua-driver.service"),
    ],
)
def test_current_autostart_file_is_a_rust_install_marker(
    tmp_path: Path,
    os_name: str,
    current_autostart_path: str,
) -> None:
    home, _calls, env = _sandbox(tmp_path, os_name)
    autostart_file = home / current_autostart_path
    autostart_file.parent.mkdir(parents=True, exist_ok=True)
    autostart_file.write_text("fixture\n", encoding="utf-8")

    skill_link = home / ".agents/skills/cua-driver"
    skill_link.parent.mkdir(parents=True)
    skill_link.symlink_to(home / ".cua-driver/skills/cua-driver")

    _run_uninstall(env)

    assert not autostart_file.exists()
    assert not skill_link.is_symlink()
