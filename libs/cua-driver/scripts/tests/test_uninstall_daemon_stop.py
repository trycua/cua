"""Coverage for the daemon-stop phase of the release Unix uninstaller.

`uninstall.sh` deletes the bundle and the runtime payloads, but Unix keeps an
unlinked binary mapped: without an explicit stop the `cua-driver serve` daemon
survives the uninstall on its deleted path. These tests pin the behaviour that
fixes that — which processes are signalled, which are deliberately left alone,
and where the phase sits relative to the TCC reset and the bundle removal.
"""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"
RELEASE_DIR_NAME = "0.19.3-aarch64-apple-darwin"
# How that directory name has to look once the uninstaller has escaped it for
# an extended regular expression.
ESCAPED_RELEASE_DIR_NAME = RELEASE_DIR_NAME.replace(".", r"\.")
# Non-root uid the sandbox pins, so a root container behaves like a developer
# machine; the root refusal itself is covered by uninstall-history-purge-test.sh.
FAKE_UID = "501"


def _write_executable(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def _sandbox(
    tmp_path: Path,
    os_name: str,
    *,
    pkill_exit: int = 1,
    pgrep_exit: int = 1,
) -> tuple[Path, Path, dict[str, str]]:
    """Run the uninstaller against a throwaway HOME with shimmed tools.

    `pkill_exit` / `pgrep_exit` stand in for "a matching process exists": 0 is
    a match, 1 is none, mirroring the real exit codes.
    """
    home = tmp_path / "home"
    fake_bin = tmp_path / "bin"
    calls = tmp_path / "calls.log"
    home.mkdir()
    fake_bin.mkdir()

    _write_executable(fake_bin / "uname", f"printf '%s\\n' '{os_name}'\n")
    # The uninstaller refuses to run as root, and it scopes every process match
    # to the invoking uid.
    _write_executable(fake_bin / "id", f"printf '%s\\n' '{FAKE_UID}'\n")
    for command in ("launchctl", "systemctl", "tccutil", "sudo", "claude"):
        _write_executable(
            fake_bin / command,
            f"printf '%s:%s\\n' '{command}' \"$*\" >> '{calls}'\n",
        )
    for command, exit_code in (("pkill", pkill_exit), ("pgrep", pgrep_exit)):
        _write_executable(
            fake_bin / command,
            f"printf '%s:%s\\n' '{command}' \"$*\" >> '{calls}'\nexit {exit_code}\n",
        )

    # Removal is confined to the sandbox HOME: anything the uninstaller asks to
    # delete outside it is recorded as a no-op so a developer machine can never
    # be reached by a test run.
    _write_executable(
        fake_bin / "rm",
        f"""for argument in "$@"; do
    case "$argument" in
        -*) ;;
        '{home}'/*)
            printf 'rm:%s\\n' "$argument" >> '{calls}'
            /bin/rm -rf -- "$argument"
            ;;
        *) printf 'rm-outside-noop:%s\\n' "$argument" >> '{calls}' ;;
    esac
done
""",
    )

    env = os.environ.copy()
    env.update({"HOME": str(home), "PATH": f"{fake_bin}:/usr/bin:/bin"})
    env.pop("CUA_DRIVER_HOME", None)
    env.pop("CUA_DRIVER_RS_HOME", None)
    return home, calls, env


def _install_rust_marker(home: Path) -> None:
    """Lay down the versioned package store the uninstaller keys off."""
    release_binary = home / ".cua-driver/packages/releases" / RELEASE_DIR_NAME / "cua-driver"
    release_binary.parent.mkdir(parents=True)
    release_binary.write_text("fixture\n", encoding="utf-8")
    release_binary.chmod(0o755)


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


def _source_only(script: str, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    """Call the uninstaller's helpers directly through its source-only seam."""
    env = {**env, "CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY": "1"}
    return subprocess.run(
        ["/bin/bash", "-c", f'source "$1"\n{script}', "bash", str(UNINSTALL)],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


@pytest.mark.parametrize("os_name", ["Darwin", "Linux"])
def test_uninstall_stops_serve_daemons_on_every_release_path(
    tmp_path: Path, os_name: str
) -> None:
    home, calls, env = _sandbox(tmp_path, os_name, pkill_exit=0)
    _install_rust_marker(home)

    result = _run_uninstall(env)

    call_lines = calls.read_text(encoding="utf-8").splitlines()
    terminated = [line for line in call_lines if line.startswith("pkill:-TERM ")]
    assert terminated, call_lines
    for binary in (
        f"{home}/\\.local/bin/cua-driver",
        "/Applications/CuaDriver\\.app/Contents/MacOS/cua-driver",
        "/Applications/CuaDriverRs\\.app/Contents/MacOS/cua-driver",
        f"{home}/\\.cua-driver/packages/current/cua-driver",
        f"{home}/\\.cua-driver-rs/packages/current/cua-driver",
        # The runtime resolves packages/current before spawning a daemon for
        # itself, so the versioned realpath has to be matched as well.
        f"{home}/\\.cua-driver/packages/releases/{ESCAPED_RELEASE_DIR_NAME}/cua-driver",
    ):
        expected = f"pkill:-TERM -U {FAKE_UID} -f ^{binary}[[:space:]]+serve([[:space:]]|$)"
        assert expected in terminated, terminated
    assert "stopped the running cua-driver serve daemon" in result.stdout


def test_serve_stop_runs_before_tcc_reset_and_bundle_removal(tmp_path: Path) -> None:
    home, _calls, env = _sandbox(tmp_path, "Darwin", pkill_exit=0)
    _install_rust_marker(home)

    lines = _run_uninstall(env).stdout.splitlines()
    stopped = next(
        index
        for index, line in enumerate(lines)
        if "stopped the running cua-driver serve daemon" in line
    )
    # tccutil resolves com.trycua.driver through LaunchServices, so the reset
    # already has to precede the bundle removal; the daemon has to stop ahead
    # of both, or the grants are revoked underneath a live process.
    revoked = next(
        index for index, line in enumerate(lines) if "revoking TCC grants" in line
    )
    bundle = next(
        index
        for index, line in enumerate(lines)
        if "/Applications/CuaDriver.app" in line
    )
    assert stopped < revoked < bundle


def test_autostart_teardown_runs_before_the_daemon_is_stopped(tmp_path: Path) -> None:
    """A KeepAlive LaunchAgent would respawn anything stopped ahead of it."""
    home, _calls, env = _sandbox(tmp_path, "Darwin", pkill_exit=0)
    _install_rust_marker(home)
    plist = home / "Library/LaunchAgents/com.trycua.cua-driver.plist"
    plist.parent.mkdir(parents=True)
    plist.write_text("fixture\n", encoding="utf-8")

    lines = _run_uninstall(env).stdout.splitlines()
    unloaded = next(
        index for index, line in enumerate(lines) if "removed LaunchAgent" in line
    )
    stopped = next(
        index
        for index, line in enumerate(lines)
        if "stopped the running cua-driver serve daemon" in line
    )
    assert unloaded < stopped


def test_no_matching_process_is_reported_as_a_skip(tmp_path: Path) -> None:
    home, calls, env = _sandbox(tmp_path, "Linux", pkill_exit=1)
    _install_rust_marker(home)

    result = _run_uninstall(env)

    assert "no running cua-driver serve daemon (skipping)" in result.stdout
    assert "stopped the running cua-driver serve daemon" not in result.stdout
    # Nothing was signalled, so nothing is escalated either.
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert not [line for line in call_lines if line.startswith("pkill:-KILL ")]


def test_swift_only_install_keeps_its_processes(tmp_path: Path) -> None:
    """No Rust marker: the shared bundle path belongs to the retired Swift
    driver, and its daemon is not ours to stop."""
    _home, calls, env = _sandbox(tmp_path, "Darwin", pkill_exit=0)

    result = _run_uninstall(env)

    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert not [line for line in call_lines if line.startswith(("pkill:", "pgrep:"))]
    assert "leaving any running cua-driver process untouched" in result.stdout


def test_surviving_mcp_children_are_reported_not_killed(tmp_path: Path) -> None:
    home, calls, env = _sandbox(tmp_path, "Linux", pkill_exit=1, pgrep_exit=0)
    _install_rust_marker(home)

    result = _run_uninstall(env)

    call_lines = calls.read_text(encoding="utf-8").splitlines()
    # The survivor probe matches argv[0] without a subcommand; every kill the
    # uninstaller issues stays scoped to `serve`.
    assert (
        f"pgrep:-U {FAKE_UID} -f ^{home}/\\.local/bin/cua-driver([[:space:]]|$)" in call_lines
    )
    signalled = [line for line in call_lines if line.startswith("pkill:")]
    assert signalled
    assert all(
        line.endswith("[[:space:]]+serve([[:space:]]|$)") for line in signalled
    ), signalled
    assert "cua-driver mcp` runs as a stdio child of your MCP client" in result.stdout


def test_argv0_patterns_are_anchored_and_metacharacters_escaped(
    tmp_path: Path,
) -> None:
    _home, _calls, env = _sandbox(tmp_path, "Linux")

    result = _source_only(
        'daemon_argv0_pattern "/opt/c.d+e(1)/cua-driver" serve\n'
        'printf "\\n"\n'
        'daemon_argv0_pattern "/opt/c.d+e(1)/cua-driver"\n',
        env,
    )

    assert result.returncode == 0, result.stderr
    serve_pattern, any_pattern = result.stdout.splitlines()
    assert serve_pattern == (
        "^/opt/c\\.d\\+e\\(1\\)/cua-driver[[:space:]]+serve([[:space:]]|$)"
    )
    assert any_pattern == "^/opt/c\\.d\\+e\\(1\\)/cua-driver([[:space:]]|$)"


def test_a_daemon_that_ignores_sigterm_is_killed(tmp_path: Path) -> None:
    _home, calls, env = _sandbox(tmp_path, "Linux", pkill_exit=0, pgrep_exit=0)

    result = _source_only('stop_release_serve_daemons "/opt/cua/cua-driver"', env)

    assert result.returncode == 0, result.stderr
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert (
        f"pkill:-KILL -U {FAKE_UID} -f ^/opt/cua/cua-driver[[:space:]]+serve([[:space:]]|$)"
        in call_lines
    )


@pytest.mark.skipif(
    shutil.which("pkill") is None or shutil.which("pgrep") is None,
    reason="pkill/pgrep are required to exercise the real matching",
)
def test_live_serve_process_is_stopped_and_mcp_child_survives(tmp_path: Path) -> None:
    """End-to-end proof that the patterns match a real process' argv[0].

    Both fixtures run the interpreter under an argv[0] of the install path, so
    their command lines are byte-identical to what a release daemon shows in
    `ps` — which is the only thing pkill/pgrep get to match on.
    """
    binary = tmp_path / "packages/releases" / RELEASE_DIR_NAME / "cua-driver"
    binary.parent.mkdir(parents=True)
    binary.write_text("fixture\n", encoding="utf-8")
    for subcommand in ("serve", "mcp"):
        (tmp_path / subcommand).write_text(
            f"import time\nopen({subcommand!r} + '.ready', 'w').write('1')\n"
            "time.sleep(600)\n",
            encoding="utf-8",
        )

    processes = {
        subcommand: subprocess.Popen(
            [str(binary), subcommand],
            executable=sys.executable,
            cwd=str(tmp_path),
        )
        for subcommand in ("serve", "mcp")
    }
    try:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if all((tmp_path / f"{name}.ready").exists() for name in processes):
                break
            time.sleep(0.1)
        else:  # pragma: no cover - only on a pathologically slow machine
            pytest.fail("fixture processes did not start")

        stop = _source_only(
            f'stop_release_serve_daemons "{binary}"\n'
            'printf "stopped=%s\\n" "$?"\n'
            f'report_surviving_release_processes "{binary}"\n'
            'printf "survivors=%s\\n" "$?"\n',
            os.environ.copy(),
        )

        assert "stopped=0" in stop.stdout, stop.stdout + stop.stderr
        assert "survivors=0" in stop.stdout, stop.stdout + stop.stderr
        assert processes["serve"].wait(timeout=30) != 0
        assert processes["mcp"].poll() is None
    finally:
        for process in processes.values():
            if process.poll() is None:
                process.kill()
                process.wait(timeout=30)
