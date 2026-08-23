"""Coverage for the daemon-stop phase of the release Unix uninstaller.

`uninstall.sh` deletes the bundle and the runtime payloads, but Unix keeps an
unlinked binary mapped: without an explicit stop the `cua-driver serve` daemon
survives the uninstall on its deleted path. These tests pin the behaviour that
fixes that — which command lines are signalled, which are deliberately left
alone, that a survivor is never reported as a stop, and where the phase sits
relative to the TCC reset and the bundle removal.

The matching assertions run the patterns the uninstaller actually issued
against realistic command lines through `grep -E`, which is the same POSIX ERE
dialect pgrep/pkill compile, rather than comparing pattern text.
"""

from __future__ import annotations

import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"
CLI_SOURCE = "libs/cua-driver/rust/crates/cua-driver/src/cli.rs"
RELEASE_DIR_NAME = "0.19.3-aarch64-apple-darwin"
# Non-root uid the sandbox pins, so a root container behaves like a developer
# machine; the root refusal itself is covered by uninstall-history-purge-test.sh.
FAKE_UID = "501"
BUNDLE_BINARY = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"


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
    release_binary = (
        home / ".cua-driver/packages/releases" / RELEASE_DIR_NAME / "cua-driver"
    )
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


def _source_only(
    script: str, env: dict[str, str], *args: str
) -> subprocess.CompletedProcess[str]:
    """Call the uninstaller's helpers directly through its source-only seam.

    Sourcing runs the script's own flag parser, which shifts the positional
    parameters away, so extra arguments are copied into `arg1`, `arg2`, … for
    the caller's snippet before the source.
    """
    env = {**env, "CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY": "1"}
    prologue = "".join(
        f'arg{index}="${index + 1}"\n' for index in range(1, len(args) + 1)
    )
    return subprocess.run(
        [
            "/bin/bash",
            "-c",
            f'{prologue}source "$1"\n{script}',
            "bash",
            str(UNINSTALL),
            *args,
        ],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


def _issued_patterns(calls: Path, signal: str = "-TERM") -> list[str]:
    """The regexes the uninstaller handed to pkill, in call order."""
    prefix = f"pkill:{signal} -U {FAKE_UID} -f "
    return [
        line[len(prefix) :]
        for line in calls.read_text(encoding="utf-8").splitlines()
        if line.startswith(prefix)
    ]


def _matched_by(patterns: list[str], command_line: str) -> bool:
    """Does any issued pattern match this command line under POSIX ERE?"""
    assert patterns, "no patterns were issued"
    script = """
line="$(cat)"
for pattern in "$@"; do
    if printf '%s' "$line" | grep -Eq "$pattern"; then
        printf 'yes'
        exit 0
    fi
done
printf 'no'
"""
    result = subprocess.run(
        ["/bin/bash", "-c", script, "bash", *patterns],
        input=command_line,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.stdout in {"yes", "no"}, result.stdout + result.stderr
    return result.stdout == "yes"


def _stopping_patterns(tmp_path: Path, os_name: str = "Darwin") -> tuple[Path, list[str]]:
    home, calls, env = _sandbox(tmp_path, os_name, pkill_exit=1)
    _install_rust_marker(home)
    _run_uninstall(env)
    return home, _issued_patterns(calls)


def _daemon_command_lines(home: Path) -> dict[str, str]:
    """Command lines a live release daemon can realistically show in `ps`."""
    return {
        # macOS: launched through `open -a`, so argv[0] is the bundle helper.
        "bundle": f"{BUNDLE_BINARY} serve",
        # The CLI takes the first non-flag argument as the subcommand, so
        # overlay/cursor flags legitimately precede `serve`.
        "bundle with bare flag": f"{BUNDLE_BINARY} --no-overlay serve",
        "bundle with value flag": (
            f"{BUNDLE_BINARY} --cursor-theme cua.default serve"
        ),
        "bundle with several flags": (
            f"{BUNDLE_BINARY} --no-overlay --socket /tmp/cua.sock serve"
            " --permission-mode bounded"
        ),
        # A value flag ahead of the subcommand consumes its own token, so the
        # subcommand is still `serve`.
        "bundle with socket flag": f"{BUNDLE_BINARY} --socket /tmp/cua.sock serve",
        "bundle with numeric value flag": f"{BUNDLE_BINARY} --glide-ms 250 serve",
        # `--flag=value` is not in VALUE_FLAGS' exact-match list, so it is an
        # ordinary flag token that consumes nothing.
        "bundle with joined value": f"{BUNDLE_BINARY} --socket=/tmp/cua.sock serve",
        "legacy bundle": (
            "/Applications/CuaDriverRs.app/Contents/MacOS/cua-driver serve"
        ),
        "cli symlink": f"{home}/.local/bin/cua-driver serve",
        "packages current": f"{home}/.cua-driver/packages/current/cua-driver serve",
        # The runtime spawns a daemon for itself through the realpath of the
        # active release.
        "versioned release": (
            f"{home}/.cua-driver/packages/releases/{RELEASE_DIR_NAME}/cua-driver serve"
        ),
        # The installer prunes old release directories, so a daemon that
        # survived an upgrade runs from a release path that no longer exists.
        "pruned release": (
            f"{home}/.cua-driver/packages/releases/0.18.0-x86_64-unknown-linux-gnu"
            "/cua-driver serve"
        ),
        "legacy home": (
            f"{home}/.cua-driver-rs/packages/current/cua-driver serve"
        ),
        # Started from a shell with the install dir on PATH: argv[0] is the
        # word as typed.
        "bare name": "cua-driver serve",
        "bare name with flag": "cua-driver --no-overlay serve",
    }


def _untouchable_command_lines(home: Path) -> dict[str, str]:
    return {
        # A stdio child of a live MCP client: it exits with its client, and
        # killing it would surface there as a transport error.
        "mcp child": f"{BUNDLE_BINARY} mcp",
        "mcp compat child": (
            f"{BUNDLE_BINARY} mcp --claude-code-computer-use-compat"
        ),
        "bare mcp child": "cua-driver mcp",
        # The separately-installed local product.
        "local product": f"{home}/.local/bin/cua-driver-local serve",
        "local packages": (
            f"{home}/.cua-driver-local/packages/current/cua-driver-local serve"
        ),
        # `curl … | bash` puts the whole uninstaller — install paths included —
        # in the launcher shell's own command line.
        "launcher shell": f"/bin/bash -c # {BUNDLE_BINARY} serve",
        "uninstaller itself": f"/bin/bash {UNINSTALL}",
        # A finite CLI call that merely names the daemon's path.
        "finite cli": f"{BUNDLE_BINARY} status --socket /tmp/cua.sock",
        # Only a VALUE_FLAGS flag swallows the token after it. `--no-overlay`
        # does not, so the subcommand here is `call` / `describe` / `status`
        # and the trailing `serve` is that command's argument — a finite CLI
        # process, not a daemon, and killing it would be a false positive.
        "call with serve argument": f"{BUNDLE_BINARY} --no-overlay call serve",
        "describe with serve argument": (
            f"{BUNDLE_BINARY} --no-overlay describe serve"
        ),
        "status with serve argument": f"{BUNDLE_BINARY} --no-overlay status serve",
        "bare flagless call": f"{BUNDLE_BINARY} call serve",
        "bare-flag call": f"{BUNDLE_BINARY} --experimental-pip call serve",
        # Another user's product outside the release install paths.
        "foreign path": "/opt/vendor/cua-driver-shim serve",
    }


@pytest.mark.parametrize("name", sorted(_daemon_command_lines(Path("/h"))))
def test_every_release_daemon_shape_is_matched(tmp_path: Path, name: str) -> None:
    home, patterns = _stopping_patterns(tmp_path)
    command_line = _daemon_command_lines(home)[name]
    assert _matched_by(patterns, command_line), command_line


@pytest.mark.parametrize("name", sorted(_untouchable_command_lines(Path("/h"))))
def test_processes_that_are_not_ours_are_never_matched(
    tmp_path: Path, name: str
) -> None:
    home, patterns = _stopping_patterns(tmp_path)
    command_line = _untouchable_command_lines(home)[name]
    assert not _matched_by(patterns, command_line), command_line


@pytest.mark.parametrize("os_name", ["Darwin", "Linux"])
def test_uninstall_signals_serve_scoped_patterns(tmp_path: Path, os_name: str) -> None:
    home, calls, env = _sandbox(tmp_path, os_name, pkill_exit=0)
    _install_rust_marker(home)

    result = _run_uninstall(env)

    issued = _issued_patterns(calls)
    assert issued
    # Every kill stays scoped to the `serve` subcommand.
    assert all(
        pattern.endswith("serve([[:space:]]|$)") for pattern in issued
    ), issued
    # …and every one is anchored at argv[0].
    assert all(pattern.startswith("^") for pattern in issued), issued
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
    assert not _issued_patterns(calls, "-KILL")


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

    # The survivor probe matches argv[0] without a subcommand, so it sees the
    # `mcp` children the kill patterns deliberately skip.
    probes = [
        line
        for line in calls.read_text(encoding="utf-8").splitlines()
        if line.startswith(f"pgrep:-U {FAKE_UID} -f ") and "serve" not in line
    ]
    assert probes
    assert "cua-driver mcp` runs as a stdio child of your MCP client" in result.stdout
    assert not _issued_patterns(calls, "-KILL")


def test_daemon_that_ignores_sigterm_is_killed_and_survival_is_reported(
    tmp_path: Path,
) -> None:
    """pkill always matches and pgrep always reports it alive: the escalation
    has to happen, and the uninstaller must not claim a stop it did not make."""
    home, calls, env = _sandbox(tmp_path, "Linux", pkill_exit=0, pgrep_exit=0)
    _install_rust_marker(home)

    result = _run_uninstall(env)

    assert _issued_patterns(calls, "-KILL")
    assert "daemon_stop_incomplete" in result.stderr
    assert "warning: could not stop the running cua-driver serve daemon" in result.stdout
    assert "stopped the running cua-driver serve daemon" not in result.stdout


def test_value_flag_list_matches_the_rust_cli(tmp_path: Path) -> None:
    """The matcher can only tell a daemon from a finite CLI call if it knows
    which flags swallow the token after them. Fail loudly when the CLI grows a
    value flag the uninstaller has not been taught about."""
    _home, _calls, env = _sandbox(tmp_path, "Linux")
    shell_flags = _source_only('printf "%s" "$DAEMON_VALUE_FLAGS"', env)
    assert shell_flags.returncode == 0, shell_flags.stderr

    source = (REPO_ROOT / CLI_SOURCE).read_text(encoding="utf-8")
    declaration = source.split("const VALUE_FLAGS: &[&str] = &[", 1)[1].split("];", 1)[0]
    rust_flags = re.findall(r'"([^"]+)"', declaration)

    assert rust_flags, "VALUE_FLAGS could not be read from the Rust CLI"
    assert shell_flags.stdout.split("|") == rust_flags


def test_verification_tool_is_required_before_signalling(tmp_path: Path) -> None:
    """Without pgrep a stop cannot be confirmed, and the phase must decline
    rather than claim one."""
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    _write_executable(fake_bin / "id", f"printf '%s\\n' '{FAKE_UID}'\n")
    _write_executable(fake_bin / "pkill", "exit 0\n")
    env = {**os.environ, "PATH": str(fake_bin), "HOME": str(tmp_path)}

    # `set -e` is live in the sourced script, so the status has to be caught.
    result = _source_only(
        'status=0\n'
        'stop_release_serve_daemons "/opt/cua/cua-driver" || status=$?\n'
        'printf "status=%s" "$status"',
        env,
    )

    assert result.stdout.endswith("status=2"), result.stdout + result.stderr


def test_argv0_patterns_escape_regex_metacharacters(tmp_path: Path) -> None:
    _home, _calls, env = _sandbox(tmp_path, "Linux")

    result = _source_only('escape_path_regex "/opt/c.d+e(1)/cua-driver"', env)

    assert result.returncode == 0, result.stderr
    assert result.stdout == "/opt/c\\.d\\+e\\(1\\)/cua-driver"


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
            'regex="$(escape_path_regex "$arg1")"\n'
            'stop_release_serve_daemons "$regex"\n'
            'printf "stopped=%s\\n" "$?"\n'
            'report_surviving_release_processes "$regex"\n'
            'printf "survivors=%s\\n" "$?"\n',
            os.environ.copy(),
            str(binary),
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
