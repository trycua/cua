"""Coverage for the daemon-stop phase of the release Unix uninstaller.

`uninstall.sh` deletes the bundle and the runtime payloads, but Unix keeps an
unlinked binary mapped: without an explicit stop the `cua-driver serve` daemon
survives the uninstall on its deleted path.

The phase makes two separate decisions, and the tests follow that split:

* which processes are *candidates* — an anchored argv[0] match over the release
  install paths, asserted here by running the patterns the uninstaller actually
  issued through `grep -E`, the same POSIX ERE dialect pgrep compiles; and
* which candidates are *daemons* — decided by scanning the real command line
  the way `positionals()` in cli.rs does, because a flag value can be the word
  `serve` (`cua-driver --socket serve mcp` is an MCP child) and no regex can
  tell those apart unambiguously.

Where the outcome is what matters, the tests spawn real processes under an
argv[0] of the install path and let the real pgrep/ps/kill run.
"""

from __future__ import annotations

import os
from pathlib import Path
import re
import shutil
import subprocess
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
# Above pid_max on Linux and macOS, so signalling it can never reach a process.
UNREACHABLE_PID = "2147483646"
STOPPED = "stopped the running cua-driver serve daemon"


def _write_executable(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def _sandbox(
    tmp_path: Path,
    os_name: str,
    *,
    record_pgrep: bool = False,
    immortal_command_line: str | None = None,
    real_uid: bool = False,
) -> tuple[Path, Path, dict[str, str]]:
    """Run the uninstaller against a throwaway HOME with shimmed tools.

    By default pgrep, ps and kill are the real ones, so process tests exercise
    the same code path a user gets. `record_pgrep` logs the patterns and
    reports no matches; `immortal_command_line` fakes a candidate that never
    dies, which is the one outcome a real process cannot be made to produce.
    """
    home = tmp_path / "home"
    fake_bin = tmp_path / "bin"
    calls = tmp_path / "calls.log"
    home.mkdir()
    fake_bin.mkdir()

    _write_executable(fake_bin / "uname", f"printf '%s\\n' '{os_name}'\n")
    if real_uid:
        # Process tests need the uid the fixtures actually run as, so `id` is
        # left alone — which also means the uninstaller's root refusal applies.
        if os.getuid() == 0:
            pytest.skip("the uninstaller refuses to run as root; run these as a user")
    else:
        # The uninstaller refuses to run as root, and it scopes every process
        # match to the invoking uid.
        _write_executable(fake_bin / "id", f"printf '%s\\n' '{FAKE_UID}'\n")
    for command in ("launchctl", "systemctl", "tccutil", "sudo", "claude"):
        _write_executable(
            fake_bin / command,
            f"printf '%s:%s\\n' '{command}' \"$*\" >> '{calls}'\n",
        )

    if record_pgrep:
        _write_executable(
            fake_bin / "pgrep",
            f"printf 'pgrep:%s\\n' \"$*\" >> '{calls}'\nexit 1\n",
        )
    elif immortal_command_line is not None:
        _write_executable(
            fake_bin / "pgrep",
            f"printf 'pgrep:%s\\n' \"$*\" >> '{calls}'\n"
            f"printf '%s\\n' '{UNREACHABLE_PID}'\n",
        )
        # A candidate that answers every liveness probe: signalling
        # UNREACHABLE_PID reaches nothing, so it never goes away.
        _write_executable(
            fake_bin / "ps",
            f"""case "$*" in
    *state*) printf 'S\\n' ;;
    *) printf '%s\\n' '{immortal_command_line}' ;;
esac
""",
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


def _issued_patterns(calls: Path) -> list[str]:
    """The argv[0] regexes the uninstaller handed to pgrep."""
    prefix = f"pgrep:-U {FAKE_UID} -f "
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


def _candidate_patterns(tmp_path: Path) -> tuple[Path, list[str]]:
    home, calls, env = _sandbox(tmp_path, "Darwin", record_pgrep=True)
    _install_rust_marker(home)
    _run_uninstall(env)
    return home, _issued_patterns(calls)


# --- argv[0] candidate selection -------------------------------------------


def _release_command_lines(home: Path) -> dict[str, str]:
    """Command lines whose argv[0] belongs to the release install."""
    return {
        # macOS: launched through `open -a`, so argv[0] is the bundle helper.
        "bundle": f"{BUNDLE_BINARY} serve",
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
        "legacy home": f"{home}/.cua-driver-rs/packages/current/cua-driver serve",
        # Started from a shell with the install dir on PATH: argv[0] is the
        # word as typed.
        "bare name": "cua-driver serve",
        # MCP children are candidates too — that is how the survivor report
        # finds them. The subcommand scan is what keeps them unsignalled.
        "mcp child": f"{BUNDLE_BINARY} mcp",
    }


def _foreign_command_lines(home: Path) -> dict[str, str]:
    """Command lines that must never even be considered."""
    return {
        # The separately-installed local product.
        "local product": f"{home}/.local/bin/cua-driver-local serve",
        "local packages": (
            f"{home}/.cua-driver-local/packages/current/cua-driver-local serve"
        ),
        # `curl … | bash` puts the whole uninstaller — install paths included —
        # in the launcher shell's own command line.
        "launcher shell": f"/bin/bash -c # {BUNDLE_BINARY} serve",
        "uninstaller itself": f"/bin/bash {UNINSTALL}",
        "foreign path": "/opt/vendor/cua-driver-shim serve",
    }


@pytest.mark.parametrize("name", sorted(_release_command_lines(Path("/h"))))
def test_release_paths_are_candidates(tmp_path: Path, name: str) -> None:
    home, patterns = _candidate_patterns(tmp_path)
    command_line = _release_command_lines(home)[name]
    assert _matched_by(patterns, command_line), command_line


@pytest.mark.parametrize("name", sorted(_foreign_command_lines(Path("/h"))))
def test_foreign_paths_are_not_candidates(tmp_path: Path, name: str) -> None:
    home, patterns = _candidate_patterns(tmp_path)
    command_line = _foreign_command_lines(home)[name]
    assert not _matched_by(patterns, command_line), command_line


# --- subcommand resolution --------------------------------------------------


SUBCOMMAND_CASES = {
    "plain": (f"{BUNDLE_BINARY} serve", "serve"),
    "bare flag first": (f"{BUNDLE_BINARY} --no-overlay serve", "serve"),
    "value flag first": (f"{BUNDLE_BINARY} --cursor-theme cua.default serve", "serve"),
    "socket value": (f"{BUNDLE_BINARY} --socket /tmp/cua.sock serve", "serve"),
    # `--flag=value` is not in VALUE_FLAGS' exact-match list, so it swallows
    # nothing.
    "joined value": (f"{BUNDLE_BINARY} --socket=/tmp/cua.sock serve", "serve"),
    "flags after the subcommand": (
        f"{BUNDLE_BINARY} --no-overlay serve --permission-mode bounded",
        "serve",
    ),
    # A value flag consumes its token, so the word `serve` here is a socket
    # name and the invocation is an MCP child of a live client.
    "serve as a socket value": (f"{BUNDLE_BINARY} --socket serve mcp", "mcp"),
    "serve as a grant value": (f"{BUNDLE_BINARY} --grant serve mcp", "mcp"),
    "serve as a socket value, status": (f"{BUNDLE_BINARY} --socket serve status", "status"),
    # A bare flag consumes nothing, so the next token is the subcommand and
    # `serve` is that command's argument.
    "serve as a call argument": (f"{BUNDLE_BINARY} --no-overlay call serve", "call"),
    "serve as a describe argument": (
        f"{BUNDLE_BINARY} --no-overlay describe serve",
        "describe",
    ),
    "flagless call": (f"{BUNDLE_BINARY} call serve", "call"),
    "mcp with trailing flag": (
        f"{BUNDLE_BINARY} mcp --claude-code-computer-use-compat",
        "mcp",
    ),
    "no subcommand at all": (BUNDLE_BINARY, ""),
}


@pytest.mark.parametrize("name", sorted(SUBCOMMAND_CASES))
def test_subcommand_scan_matches_the_cli(tmp_path: Path, name: str) -> None:
    """The scan has to agree with `positionals()` in cli.rs, in both
    directions: a missed daemon is the bug this phase fixes, and a false
    daemon is a process the uninstaller kills that it must not touch."""
    command_line, expected = SUBCOMMAND_CASES[name]
    _home, _calls, env = _sandbox(tmp_path, "Linux", record_pgrep=True)

    result = _source_only(
        'daemon_subcommand "$arg1" || true', env, command_line
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == expected


def test_value_flag_list_matches_the_rust_cli(tmp_path: Path) -> None:
    """The scan can only tell a daemon from a finite CLI call if it knows which
    flags swallow the token after them. Fail loudly when the CLI grows a value
    flag the uninstaller has not been taught about."""
    _home, _calls, env = _sandbox(tmp_path, "Linux", record_pgrep=True)
    shell_flags = _source_only('printf "%s" "$DAEMON_VALUE_FLAGS"', env)
    assert shell_flags.returncode == 0, shell_flags.stderr

    source = (REPO_ROOT / CLI_SOURCE).read_text(encoding="utf-8")
    declaration = source.split("const VALUE_FLAGS: &[&str] = &[", 1)[1].split("];", 1)[0]
    rust_flags = re.findall(r'"([^"]+)"', declaration)

    assert rust_flags, "VALUE_FLAGS could not be read from the Rust CLI"
    assert shell_flags.stdout.split() == rust_flags


# --- live processes ---------------------------------------------------------


@pytest.fixture(scope="session")
def sleeper(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """A binary that ignores its arguments and blocks.

    Compiled rather than scripted because the whole point is to control argv:
    an interpreter would parse `--socket` itself, and a `#!` script would put
    the interpreter in argv[0] where the install path has to be.
    """
    compiler = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    if compiler is None:
        pytest.skip("no C compiler available to build the argv fixture")
    directory = tmp_path_factory.mktemp("sleeper")
    source = directory / "sleeper.c"
    source.write_text("#include <unistd.h>\nint main(void){pause();return 0;}\n")
    binary = directory / "sleeper"
    subprocess.run(
        [compiler, str(source), "-o", str(binary)], check=True, capture_output=True
    )
    return binary


class _Fixtures:
    """Processes spawned under an argv[0] of an install path."""

    def __init__(self, sleeper: Path) -> None:
        self._sleeper = sleeper
        self.processes: dict[str, subprocess.Popen[bytes]] = {}

    def spawn(self, name: str, argv0: Path | str, *arguments: str) -> int:
        process = subprocess.Popen(
            [str(argv0), *arguments], executable=str(self._sleeper)
        )
        self.processes[name] = process
        return process.pid

    def alive(self, name: str) -> bool:
        process = self.processes[name]
        if process.poll() is not None:
            return False
        # A killed-but-unreaped child is not alive; reap what has exited.
        try:
            process.wait(timeout=0)
        except subprocess.TimeoutExpired:
            return True
        return False

    def wait_started(self) -> None:
        # Popen has already forked; give the exec a moment to land so the new
        # command line is visible to ps/pgrep.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            listed = subprocess.run(
                ["ps", "-ww", "-o", "args=", "-p"]
                + [",".join(str(p.pid) for p in self.processes.values())],
                text=True,
                capture_output=True,
                check=False,
            ).stdout
            if listed.count("\n") >= len(self.processes):
                return
            time.sleep(0.1)
        raise AssertionError("fixture processes did not start")

    def cleanup(self) -> None:
        for process in self.processes.values():
            if process.poll() is None:
                process.kill()
            process.wait(timeout=30)


@pytest.fixture
def fixtures(sleeper: Path):
    running = _Fixtures(sleeper)
    try:
        yield running
    finally:
        running.cleanup()


def test_only_serve_invocations_are_stopped(tmp_path: Path, fixtures) -> None:
    """The regression that matters: every shape below has the same argv[0], so
    the kill decision rests entirely on the subcommand scan."""
    binary = tmp_path / "packages/releases" / RELEASE_DIR_NAME / "cua-driver"
    fixtures.spawn("plain serve", binary, "serve")
    fixtures.spawn("flagged serve", binary, "--no-overlay", "serve")
    fixtures.spawn("socket serve", binary, "--socket", "/tmp/cua.sock", "serve")
    # An MCP child whose socket is named `serve`, and a call whose argument is.
    fixtures.spawn("mcp with serve socket", binary, "--socket", "serve", "mcp")
    fixtures.spawn("call with serve argument", binary, "--no-overlay", "call", "serve")
    fixtures.spawn("plain mcp", binary, "mcp")
    fixtures.wait_started()

    stop = _source_only(
        'status=0\n'
        'regex="$(escape_path_regex "$arg1")"\n'
        'stop_release_serve_daemons "$regex" || status=$?\n'
        'printf "stopped=%s\\n" "$status"\n'
        'report_surviving_release_processes "$regex" && printf "survivors=yes\\n"\n',
        os.environ.copy(),
        str(binary),
    )

    assert "stopped=0" in stop.stdout, stop.stdout + stop.stderr
    assert not fixtures.alive("plain serve")
    assert not fixtures.alive("flagged serve")
    assert not fixtures.alive("socket serve")
    assert fixtures.alive("mcp with serve socket")
    assert fixtures.alive("call with serve argument")
    assert fixtures.alive("plain mcp")
    assert "survivors=yes" in stop.stdout


def test_uninstall_stops_a_live_daemon_and_reports_it(
    tmp_path: Path, fixtures
) -> None:
    home, _calls, env = _sandbox(tmp_path, "Linux", real_uid=True)
    _install_rust_marker(home)
    fixtures.spawn(
        "daemon", home / ".cua-driver/packages/current/cua-driver", "--no-overlay", "serve"
    )
    fixtures.wait_started()

    result = _run_uninstall(env)

    assert STOPPED in result.stdout
    assert not fixtures.alive("daemon")


def test_live_mcp_child_is_reported_not_killed(tmp_path: Path, fixtures) -> None:
    home, _calls, env = _sandbox(tmp_path, "Linux", real_uid=True)
    _install_rust_marker(home)
    fixtures.spawn(
        "mcp", home / ".cua-driver/packages/current/cua-driver", "--socket", "serve", "mcp"
    )
    fixtures.wait_started()

    result = _run_uninstall(env)

    assert fixtures.alive("mcp")
    assert STOPPED not in result.stdout
    assert "no running cua-driver serve daemon (skipping)" in result.stdout
    assert "cua-driver mcp` runs as a stdio child of your MCP client" in result.stdout


# --- ordering, gating and failure reporting ---------------------------------


def test_serve_stop_runs_before_tcc_reset_and_bundle_removal(
    tmp_path: Path, fixtures
) -> None:
    home, _calls, env = _sandbox(tmp_path, "Darwin", real_uid=True)
    _install_rust_marker(home)
    fixtures.spawn("daemon", home / ".cua-driver/packages/current/cua-driver", "serve")
    fixtures.wait_started()

    lines = _run_uninstall(env).stdout.splitlines()
    stopped = next(index for index, line in enumerate(lines) if STOPPED in line)
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


def test_autostart_teardown_runs_before_the_daemon_is_stopped(
    tmp_path: Path, fixtures
) -> None:
    """A KeepAlive LaunchAgent would respawn anything stopped ahead of it."""
    home, _calls, env = _sandbox(tmp_path, "Darwin", real_uid=True)
    _install_rust_marker(home)
    plist = home / "Library/LaunchAgents/com.trycua.cua-driver.plist"
    plist.parent.mkdir(parents=True)
    plist.write_text("fixture\n", encoding="utf-8")
    fixtures.spawn("daemon", home / ".cua-driver/packages/current/cua-driver", "serve")
    fixtures.wait_started()

    lines = _run_uninstall(env).stdout.splitlines()
    unloaded = next(
        index for index, line in enumerate(lines) if "removed LaunchAgent" in line
    )
    stopped = next(index for index, line in enumerate(lines) if STOPPED in line)
    assert unloaded < stopped


def test_no_matching_process_is_reported_as_a_skip(tmp_path: Path) -> None:
    home, _calls, env = _sandbox(tmp_path, "Linux")
    _install_rust_marker(home)

    result = _run_uninstall(env)

    assert "no running cua-driver serve daemon (skipping)" in result.stdout
    assert STOPPED not in result.stdout


def test_swift_only_install_keeps_its_processes(tmp_path: Path) -> None:
    """No Rust marker: the shared bundle path belongs to the retired Swift
    driver, and its daemon is not ours to stop."""
    _home, calls, env = _sandbox(tmp_path, "Darwin", record_pgrep=True)

    result = _run_uninstall(env)

    assert not calls.read_text(encoding="utf-8").count("pgrep:")
    assert "leaving any running cua-driver process untouched" in result.stdout


def test_a_daemon_that_survives_sigkill_is_never_reported_as_stopped(
    tmp_path: Path,
) -> None:
    """The one outcome a real process cannot produce, faked: a candidate that
    answers every liveness probe."""
    home, _calls, env = _sandbox(
        tmp_path,
        "Linux",
        immortal_command_line=f"{BUNDLE_BINARY} serve",
    )
    _install_rust_marker(home)

    result = _run_uninstall(env)

    assert "daemon_stop_incomplete" in result.stderr
    assert "warning: could not stop the running cua-driver serve daemon" in result.stdout
    assert STOPPED not in result.stdout


def test_process_tools_are_required_before_signalling(tmp_path: Path) -> None:
    """Without pgrep and ps a stop can be neither targeted nor confirmed, and
    the phase must decline rather than claim one."""
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    _write_executable(fake_bin / "id", f"printf '%s\\n' '{FAKE_UID}'\n")
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
    _home, _calls, env = _sandbox(tmp_path, "Linux", record_pgrep=True)

    result = _source_only('escape_path_regex "/opt/c.d+e(1)/cua-driver"', env)

    assert result.returncode == 0, result.stderr
    assert result.stdout == "/opt/c\\.d\\+e\\(1\\)/cua-driver"
