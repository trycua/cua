"""A Cua Driver daemon from ANY copy of the app gets stopped, and nothing else does.

A Mac can hold a second CuaDriver.app (an old release in ~/Downloads, the Trash, a
benchmark folder). `open -a CuaDriver` may start that copy, so the daemon is not
the installed file and not in the pid file. The uninstaller and the installer's
swap step must still identify it by bundle id (com.trycua.driver), stop it
gracefully, then SIGTERM, then SIGKILL, say what they stopped, and never touch
other users' processes or other apps.

Safety: every "daemon" here is a throwaway `sleep` that this test spawns. `ps`,
`pgrep` and process identity are faked, so the tests can never see, let alone
signal, a real Cua Driver daemon running on the machine.
"""

from __future__ import annotations

import os
import re
import signal
import subprocess
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[4]
SCRIPTS = ROOT / "libs/cua-driver/scripts"
UNINSTALL = SCRIPTS / "uninstall.sh"
COMMON = SCRIPTS / "_install-common.sh"

PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>cua-driver</string>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
</dict>
</plist>
"""


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


class Machine:
    """Fake processes in fake app bundles, with `ps`/`pgrep` shims over them."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.bin = root / "bin"
        self.fake = root / "fake"
        self.home = root / "home"
        for directory in (self.bin, self.fake, self.home):
            directory.mkdir()
        self.pids: list[int] = []
        self.pgrep_log = root / "pgrep.log"
        self._write(
            self.bin / "pgrep",
            f'echo "$@" >> "{self.pgrep_log}"\n'
            f'for f in "{self.fake}"/*.cmd; do [ -e "$f" ] || continue; '
            'pid="$(basename "$f" .cmd)"; kill -0 "$pid" 2>/dev/null && echo "$pid"; done; exit 0',
        )
        # ps: only used for ancestors (-o ppid=) and the installer's listing.
        self._write(
            self.bin / "ps",
            f'case "$*" in\n'
            f'  *ppid=*) exit 0 ;;\n'
            f'  *"-o pid="*)\n'
            f'    for f in "{self.fake}"/*.cmd; do [ -e "$f" ] || continue; '
            'pid="$(basename "$f" .cmd)"; kill -0 "$pid" 2>/dev/null && '
            'printf "%s %s\\n" "$pid" "$(cat "$f")"; done; exit 0 ;;\n'
            f'  *"-o command="*)\n'
            f'    for a in "$@"; do pid="$a"; done\n'
            f'    cat "{self.fake}/$pid.cmd" 2>/dev/null; exit 0 ;;\n'
            f'esac\nexit 2',
        )
        for name, body in {
            "id": 'if [ "$1" = -u ]; then echo 501; else /usr/bin/id "$@"; fi',
            "pkill": f'echo pkill "$@" >> "{root}/pkill.log"',
        }.items():
            self._write(self.bin / name, body)

    @staticmethod
    def _write(path: Path, body: str) -> None:
        path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
        path.chmod(0o755)

    def bundle(
        self,
        where: str,
        bundle_id: str | None = "com.trycua.driver",
        version: str = "0.34.0",
        helper: str = "exit 1",
    ) -> Path:
        """An app bundle whose cua-driver helper runs `helper` (a shell body)."""
        app = self.root / where
        (app / "Contents/MacOS").mkdir(parents=True)
        if bundle_id is not None:
            (app / "Contents/Info.plist").write_text(
                PLIST.format(bundle_id=bundle_id, version=version), encoding="utf-8"
            )
        self._write(app / "Contents/MacOS/cua-driver", helper)
        return app

    def spawn(self, app: Path, ignore_term: bool = False) -> int:
        """Start a throwaway process that the fakes report as running from `app`."""
        guard = 'trap "" TERM; ' if ignore_term else ""
        # The inner sh exits at once, so the worker is re-parented and never
        # lingers as a zombie that `kill -0` would still see.
        out = subprocess.run(
            ["/bin/sh", "-c", f"({guard}exec sleep 300) >/dev/null 2>&1 & echo $!"],
            capture_output=True,
            text=True,
            check=True,
        )
        pid = int(out.stdout.strip())
        self.pids.append(pid)
        (self.fake / f"{pid}.cmd").write_text(
            f"{app}/Contents/MacOS/cua-driver serve", encoding="utf-8"
        )
        return pid

    def spawn_exit_after(self, pid: int, seconds: int) -> None:
        """Make the fake process `pid` exit by itself after `seconds`."""
        subprocess.Popen(
            ["/bin/sh", "-c", f"sleep {seconds}; kill {pid}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )

    def wait_dead(self, pid: int, seconds: float = 3.0) -> bool:
        deadline = time.time() + seconds
        while time.time() < deadline:
            if not alive(pid):
                return True
            time.sleep(0.05)
        return not alive(pid)

    def cleanup(self) -> None:
        for pid in self.pids:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def env(self, **extra: str) -> dict[str, str]:
        env = {k: v for k, v in os.environ.items() if not k.startswith("CUA_DRIVER_")}
        env.update(
            HOME=str(self.home),
            PATH=f"{self.bin}:/usr/bin:/bin",
            CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY="1",
            CUA_DRIVER_STOP_TIMEOUT="1",
        )
        env.update(extra)
        return env

    def uninstall_shell(self, body: str) -> subprocess.CompletedProcess[str]:
        """Source uninstall.sh and run `body` with process identity read from the fakes."""
        prelude = f"""
APP_BUNDLE=/Applications/CuaDriver.app
LEGACY_APP_BUNDLE=/Applications/CuaDriverRs.app
HOME_DIR="$HOME/.cua-driver"
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
USER_BIN_LINK="$HOME/.local/bin/cua-driver"
DAEMON_PID_FILE=""
DAEMON_STOP_HELPER=""
daemon_process_identity() {{ cat "{self.fake}/$1.cmd" 2>/dev/null || return 2; }}
daemon_process_generation() {{ printf 'generation-%s' "$1"; }}
"""
        return subprocess.run(
            ["/bin/bash", "-c", f'source "{UNINSTALL}"\n{prelude}\n{body}'],
            cwd=ROOT,
            env=self.env(),
            text=True,
            capture_output=True,
            check=False,
            timeout=60,
        )

    def common_shell(self, body: str, **env: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["/bin/bash", "-c", f'source "{COMMON}"\n{body}'],
            cwd=ROOT,
            env=self.env(**env),
            text=True,
            capture_output=True,
            check=False,
            timeout=60,
        )


@pytest.fixture
def machine(tmp_path: Path):
    m = Machine(tmp_path)
    yield m
    m.cleanup()


def test_uninstaller_stops_a_daemon_from_another_copy_of_the_app(machine: Machine) -> None:
    old = machine.bundle("Old Apps/CuaDriver-0.34.0.app", version="0.34.0")
    stray = machine.spawn(old)

    result = machine.uninstall_shell("stop_release_daemon")

    assert result.returncode == 0, result.stdout + result.stderr
    assert machine.wait_dead(stray), "the stray daemon is still running"
    assert f"stopped Cua Driver (pid {stray}, version 0.34.0, {old}) with SIGTERM" in result.stdout
    assert "daemon_stop_incomplete" not in result.stderr
    assert old.exists(), "stopping a daemon must not delete the app"


def test_uninstaller_tries_a_graceful_stop_through_the_copys_own_cli_first(machine: Machine) -> None:
    old = machine.bundle(
        "Downloads/CuaDriver.app",
        helper='[ "$1" = --expected-pid ] && [ "$3" = stop ] && kill "$2"',
    )
    stray = machine.spawn(old)

    result = machine.uninstall_shell("stop_release_daemon")

    assert result.returncode == 0, result.stdout + result.stderr
    assert machine.wait_dead(stray)
    assert "with a graceful stop request" in result.stdout


def test_uninstaller_escalates_to_sigkill_when_sigterm_is_ignored(machine: Machine) -> None:
    old = machine.bundle(".Trash/CuaDriver.app")
    stubborn = machine.spawn(old, ignore_term=True)

    result = machine.uninstall_shell("stop_release_daemon")

    assert result.returncode == 0, result.stdout + result.stderr
    assert machine.wait_dead(stubborn), "SIGKILL fallback never fired"
    assert "with SIGKILL after it ignored SIGTERM for 1s" in result.stdout


def test_uninstaller_stops_every_copy_but_never_other_apps(machine: Machine) -> None:
    old_a = machine.bundle("Downloads/CuaDriver.app", version="0.34.0")
    old_b = machine.bundle("bench/CuaDriver.app", version="0.36.0")
    deleted = machine.bundle("gone/CuaDriver-0.35.0.app", bundle_id=None)
    local = machine.bundle("Applications/CuaDriverLocal.app", bundle_id="com.trycua.driver.local")
    other_app = machine.bundle("Applications/Other.app", bundle_id="com.example.other")
    unreadable_other = machine.bundle("Mystery.app", bundle_id=None)

    mine = [machine.spawn(old_a), machine.spawn(old_b), machine.spawn(deleted)]
    not_mine = [machine.spawn(local), machine.spawn(other_app), machine.spawn(unreadable_other)]

    result = machine.uninstall_shell("stop_release_daemon")

    assert result.returncode == 0, result.stdout + result.stderr
    for pid in mine:
        assert machine.wait_dead(pid), f"Cua Driver pid {pid} survived"
    for pid in not_mine:
        assert alive(pid), f"pid {pid} belongs to another app and must not be stopped"
    assert result.stdout.count("stopped Cua Driver (") == 3


def test_uninstaller_only_looks_at_the_current_users_processes(machine: Machine) -> None:
    machine.uninstall_shell("release_daemon_fallback_pids")

    # pgrep is asked for this user's processes only (-U <uid>); another user's
    # cua-driver is never even a candidate.
    assert re.search(r"-U \d+", machine.pgrep_log.read_text())


def test_uninstaller_never_selects_its_own_ancestors(machine: Machine) -> None:
    old = machine.bundle("Downloads/CuaDriver.app")
    caller = machine.spawn(old)  # e.g. `cua-driver update` that started the uninstaller
    other = machine.spawn(old)

    result = machine.uninstall_shell(
        f'cua_driver_ancestor_pids() {{ echo {caller}; }}\nrelease_daemon_fallback_pids'
    )

    assert result.stdout.split() == [str(other)]
    assert alive(caller)


def test_uninstaller_fails_closed_and_names_a_process_it_cannot_stop(machine: Machine) -> None:
    old = machine.bundle("Downloads/CuaDriver.app")
    machine.spawn(old)

    # Signals that cannot be delivered: the process survives every step.
    result = machine.uninstall_shell(
        "daemon_signal_if_current() { return 1; }\nstop_release_daemon"
    )

    assert result.returncode == 1
    assert "daemon_stop_incomplete: Cua Driver process(es) still running" in result.stderr


def test_installer_swap_step_stops_a_daemon_from_another_copy(machine: Machine) -> None:
    old = machine.bundle("tmp/CuaDriver.app", version="0.36.0")
    stray = machine.spawn(old)

    result = machine.common_shell("cua_driver_stop_release_processes")

    assert result.returncode == 0, result.stdout + result.stderr
    assert machine.wait_dead(stray)
    assert f"stopped Cua Driver (pid {stray}, version 0.36.0, {old}) with SIGTERM" in result.stdout


def test_installer_swap_step_never_touches_other_apps_and_skips_pkill_on_macos(machine: Machine) -> None:
    other_app = machine.bundle("Applications/Spaces.app", bundle_id="com.example.spaces")
    local = machine.bundle("Applications/CuaDriverLocal.app", bundle_id="com.trycua.driver.local")
    bystanders = [machine.spawn(other_app), machine.spawn(local)]
    machine._write(machine.bin / "uname", "echo Darwin")
    machine._write(machine.bin / "launchctl", "exit 0")

    result = machine.common_shell("stop_cua_driver_daemons")

    assert result.returncode == 0, result.stdout + result.stderr
    for pid in bystanders:
        assert alive(pid)
    assert not (machine.root / "pkill.log").exists(), (
        "macOS must not pkill by name: another app's helper may share the name cua-driver"
    )


def test_installer_survivor_note_names_the_pid(machine: Machine) -> None:
    old = machine.bundle("tmp/CuaDriver.app")
    stray = machine.spawn(old)
    machine._write(machine.bin / "uname", "echo Darwin")

    result = machine.common_shell("show_cua_driver_daemon_survivors")

    assert f"(pid: {stray})" in result.stdout


def test_a_short_lived_stop_helper_process_is_not_reported_as_a_survivor(machine: Machine) -> None:
    """`cua-driver --expected-pid N stop` re-executes itself; it is not a daemon left behind."""
    old = machine.bundle("tmp/CuaDriver.app")
    helper = machine.spawn(old)  # exits by itself in ~1s, like the stop helper chain
    machine.spawn_exit_after(helper, 1)
    machine._write(machine.bin / "uname", "echo Darwin")

    result = machine.common_shell("show_cua_driver_daemon_survivors", CUA_DRIVER_STOP_TIMEOUT="4")

    assert result.stdout == ""
    assert machine.wait_dead(helper)


def test_uninstaller_waits_for_a_short_lived_helper_before_failing(machine: Machine) -> None:
    old = machine.bundle("Downloads/CuaDriver.app")
    helper = machine.spawn(old)
    machine.spawn_exit_after(helper, 1)

    result = machine.uninstall_shell(
        "CUA_DRIVER_STOP_TIMEOUT=4\nverify_release_daemon_absent"
    )

    assert result.returncode == 0, result.stdout + result.stderr


def _function_body(script: Path, name: str) -> str:
    match = re.search(rf"^{name}\(\) \{{\n.*?^\}}\n", script.read_text(), re.S | re.M)
    assert match, f"{name} missing from {script.name}"
    return match.group(0)


@pytest.mark.parametrize(
    "name",
    [
        "cua_driver_plist_string",
        "cua_driver_bundle_of_command",
        "cua_driver_bundle_is_release",
        "cua_driver_ancestor_pids",
        "cua_driver_run_bounded",
    ],
)
def test_uninstall_copy_of_each_shared_helper_has_not_drifted(name: str) -> None:
    """uninstall.sh runs standalone (curl | bash), so it carries verbatim copies."""
    assert _function_body(UNINSTALL, name) == _function_body(COMMON, name)
