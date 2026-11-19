"""The installer connects agents and runs the self-test after installing.

Runs the real post-install block of `_install-rust.sh` against a fake
`cua-driver` so each guard is exercised: an older binary without `connect` /
`test`, the `--no-connect` / `--no-test` flags and their env vars, and the
"no terminal" case that skips the permission guide.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1]
INSTALLER = SCRIPTS / "_install-rust.sh"

requires_posix_bash = pytest.mark.skipif(
    os.name != "posix" or shutil.which("bash") is None,
    reason="requires a posix host with bash",
)


def _post_install_block() -> str:
    source = INSTALLER.read_text(encoding="utf-8")
    start = source.index("driver_has_command() {")
    end = source.index("# Unified post-install hints come from")
    return source[start:end]


def _fake_driver(tmp_path: Path, subcommands: str, connect_out: str, connect_rc: int = 0) -> Path:
    fake = tmp_path / "cua-driver"
    log = tmp_path / "calls.log"
    fake.write_text(
        textwrap.dedent(f"""\
            #!/bin/sh
            echo "$@" >> '{log}'
            case "$1" in
              --help) echo "cua-driver 9.9.9"; echo "Subcommands: {subcommands}" ;;
              connect) printf '%s\\n' '{connect_out}'; exit {connect_rc} ;;
              test) echo "Cua Driver works." ;;
              permissions) echo "grant ran" ;;
            esac
            exit 0
        """),
        encoding="utf-8",
    )
    fake.chmod(0o755)
    return fake


def _run(tmp_path: Path, fake: Path, flags: str = "", env: dict[str, str] | None = None):
    script = tmp_path / "harness.sh"
    script.write_text(
        "set -euo pipefail\n"
        f'BIN_LINK="{fake}"\n'
        f"NO_CONNECT=\"${{CUA_DRIVER_NO_CONNECT:-0}}\"\nNO_TEST=\"${{CUA_DRIVER_NO_TEST:-0}}\"\n"
        f"{flags}\n"
        + _post_install_block()
        + '\necho "FELL_THROUGH_TO_HINTS"\n',
        encoding="utf-8",
    )
    full_env = {k: v for k, v in os.environ.items() if not k.startswith("CUA_DRIVER_NO_")}
    full_env.pop("CI", None)
    full_env.update(env or {})
    result = subprocess.run(
        ["bash", str(script)],
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
        env=full_env,
        check=False,
    )
    calls = (tmp_path / "calls.log").read_text().splitlines() if (tmp_path / "calls.log").exists() else []
    return result, calls


NEW = "mcp, list-tools, test, connect, mcp-config"


@requires_posix_bash
def test_connects_tests_and_ends_with_one_done_line(tmp_path: Path) -> None:
    fake = _fake_driver(tmp_path, NEW, "Claude Code: connected\nCodex: not installed")
    result, calls = _run(tmp_path, fake)
    assert result.returncode == 0, result.stderr
    assert "Claude Code: connected" in result.stdout
    assert "Cua Driver works." in result.stdout
    assert 'Done. Ask Claude Code' in result.stdout
    assert "FELL_THROUGH_TO_HINTS" not in result.stdout
    assert "connect" in calls and "test" in calls
    # No terminal on stdin, so the permission guide must not run.
    assert "permissions grant" not in calls


@requires_posix_bash
def test_names_the_agent_that_was_actually_connected(tmp_path: Path) -> None:
    fake = _fake_driver(tmp_path, NEW, "Claude Code: not installed\nCodex: already connected")
    result, _ = _run(tmp_path, fake)
    assert "Done. Ask Codex" in result.stdout


@requires_posix_bash
def test_no_agent_connected_falls_back_to_the_hints(tmp_path: Path) -> None:
    fake = _fake_driver(tmp_path, NEW, "Claude Code: not installed\nCodex: not installed")
    result, _ = _run(tmp_path, fake)
    assert "Done. Ask" not in result.stdout
    assert "FELL_THROUGH_TO_HINTS" in result.stdout


@requires_posix_bash
def test_failed_connect_falls_back_to_the_hints(tmp_path: Path) -> None:
    fake = _fake_driver(tmp_path, NEW, "Claude Code: could not connect (boom)", connect_rc=1)
    result, _ = _run(tmp_path, fake)
    assert result.returncode == 0
    assert "FELL_THROUGH_TO_HINTS" in result.stdout


@requires_posix_bash
def test_older_binary_without_connect_or_test_is_left_alone(tmp_path: Path) -> None:
    fake = _fake_driver(tmp_path, "mcp, list-tools, mcp-config", "")
    result, calls = _run(tmp_path, fake)
    assert result.returncode == 0
    assert "FELL_THROUGH_TO_HINTS" in result.stdout
    assert "connect" not in calls and "test" not in calls


@requires_posix_bash
@pytest.mark.parametrize(
    ("flags", "env"),
    [("NO_CONNECT=1", {}), ("", {"CUA_DRIVER_NO_CONNECT": "1"})],
)
def test_no_connect_skips_connect(tmp_path: Path, flags: str, env: dict[str, str]) -> None:
    fake = _fake_driver(tmp_path, NEW, "Claude Code: connected")
    result, calls = _run(tmp_path, fake, flags, env)
    assert "connect" not in calls
    assert "FELL_THROUGH_TO_HINTS" in result.stdout


@requires_posix_bash
@pytest.mark.parametrize(
    ("flags", "env"),
    [("NO_TEST=1", {}), ("", {"CUA_DRIVER_NO_TEST": "1"})],
)
def test_no_test_skips_the_self_test(tmp_path: Path, flags: str, env: dict[str, str]) -> None:
    fake = _fake_driver(tmp_path, NEW, "Claude Code: connected")
    result, calls = _run(tmp_path, fake, flags, env)
    assert "test" not in calls
    assert "connect" in calls
    assert "Done. Ask Claude Code" in result.stdout


def test_flags_are_parsed_and_documented() -> None:
    source = INSTALLER.read_text(encoding="utf-8")
    assert "--no-connect) NO_CONNECT=1; shift ;;" in source
    assert "--no-test) NO_TEST=1; shift ;;" in source
    header = source.split("set -euo pipefail", 1)[0]
    for needle in ("--no-connect", "--no-test", "CUA_DRIVER_NO_CONNECT=1", "CUA_DRIVER_NO_TEST=1"):
        assert needle in header
    wrapper = (SCRIPTS / "install.sh").read_text(encoding="utf-8").split("set -euo pipefail", 1)[0]
    for needle in ("--no-connect", "--no-test", "CUA_DRIVER_NO_CONNECT=1", "CUA_DRIVER_NO_TEST=1"):
        assert needle in wrapper


def test_permission_guide_needs_a_macos_terminal_outside_ci() -> None:
    block = _post_install_block()
    guard = next(line for line in block.splitlines() if "permissions grant" not in line and "Darwin" in line)
    for needle in ('"$(uname -s)" == "Darwin"', "-t 0", "-t 1", '-z "${CI:-}"'):
        assert needle in guard


def test_every_new_call_uses_the_absolute_binary_path() -> None:
    block = _post_install_block()
    assert "cua-driver connect" not in block and "cua-driver test" not in block
    for call in ('"$BIN_LINK" connect', '"$BIN_LINK" test', '"$BIN_LINK" permissions grant'):
        assert call in block


def test_windows_installer_connects_and_tests_with_guards_and_no_permission_step() -> None:
    source = (SCRIPTS / "install.ps1").read_text(encoding="utf-8-sig")
    assert "[switch]$NoConnect" in source and "[switch]$NoTest" in source
    assert "$env:CUA_DRIVER_NO_CONNECT -eq '1'" in source
    assert "$env:CUA_DRIVER_NO_TEST -eq '1'" in source
    assert "& $installedBinary connect" in source
    assert "& $installedBinary test" in source
    assert "Test-CuaDriverCommand $installedBinary 'connect'" in source
    assert "Test-CuaDriverCommand $installedBinary 'test'" in source
    assert "permissions grant" not in source.split("function Test-CuaDriverCommand")[1].split("Unified post-install hints")[0]
    assert "if (-not $SkipHints)" in source


def test_post_install_hints_lead_with_connect() -> None:
    hints = (SCRIPTS / "post-install-hints.txt").read_text(encoding="utf-8")
    assert hints.index("{{BINARY}} connect") < hints.index("permissions grant")
    assert "mcp-config --client <name>" in hints
