"""Focused coverage for fail-closed Unix release daemon shutdown."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = ROOT / "libs/cua-driver/scripts/uninstall.sh"


def executable(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


class DaemonStopTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.bin = self.root / "bin"
        self.home.mkdir()
        self.bin.mkdir()
        self.env = {
            **os.environ,
            "HOME": str(self.home),
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY": "1",
        }

    def tearDown(self) -> None:
        self.temp.cleanup()

    def shell(self, body: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["/bin/bash", "-c", f'source "{UNINSTALL}"\n{body}'],
            cwd=ROOT,
            env=self.env,
            text=True,
            capture_output=True,
            check=False,
        )

    def setup(self, helper: Path | None) -> str:
        pid_file = self.home / "daemon.pid"
        pid_file.write_text("4242\n", encoding="utf-8")
        return f"""
APP_BUNDLE=/Applications/CuaDriver.app
LEGACY_APP_BUNDLE=/Applications/CuaDriverRs.app
HOME_DIR="$HOME/.cua-driver"
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
DAEMON_PID_FILE="{pid_file}"
DAEMON_STOP_HELPER="{helper or ''}"
daemon_pid_alive() {{ return 0; }}
daemon_pid_is_release() {{ return 0; }}
daemon_process_generation() {{ printf generation-1; }}
verify_release_daemon_absent() {{ return 0; }}
"""

    def test_helper_receives_only_guarded_stop(self) -> None:
        marker = self.root / "argv"
        helper = self.home / "cua-driver"
        executable(helper, f"printf '%s|' \"$@\" > '{marker}'")
        result = self.shell(
            self.setup(helper)
            + """
daemon_wait_for_exit() { return 0; }
stop_release_daemon
"""
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(marker.read_text(), "--expected-pid|4242|stop|")

    def test_old_helper_falls_back_without_plain_stop(self) -> None:
        unbound = self.root / "unbound"
        signals = self.root / "signals"
        waits = self.root / "waits"
        helper = self.home / "old-cua-driver"
        executable(helper, f'[ "$1" != stop ] || : > "{unbound}"\nexit 2')
        result = self.shell(
            self.setup(helper)
            + f"""
daemon_wait_for_exit() {{
  count=0; [[ ! -f "{waits}" ]] || count=$(cat "{waits}")
  count=$((count + 1)); printf '%s' "$count" > "{waits}"
  [[ "$count" -gt 1 ]]
}}
daemon_signal_if_current() {{ printf '%s' "$3" > "{signals}"; return 0; }}
stop_release_daemon
"""
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(unbound.exists())
        self.assertEqual(signals.read_text(), "TERM")

    def test_reused_pid_is_not_signalled(self) -> None:
        helper = self.home / "cua-driver"
        executable(helper, "exit 1")
        result = self.shell(
            self.setup(helper)
            + """
daemon_wait_for_exit() { return 1; }
daemon_signal_if_current() { return 1; }
kill() { return 99; }
stop_release_daemon
"""
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_helper_fails_before_signal(self) -> None:
        result = self.shell(self.setup(None) + "stop_release_daemon")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("stop helper is unavailable", result.stderr)

    def test_process_generation_guards_signal(self) -> None:
        marker = self.root / "signal"
        result = self.shell(
            self.setup(None)
            + f"""
daemon_pid_is_release() {{ return 0; }}
daemon_process_generation() {{ printf "$GENERATION"; }}
kill() {{ printf '%s' "$*" > "{marker}"; }}
GENERATION=old
daemon_signal_if_current 4242 old TERM
GENERATION=new
daemon_signal_if_current 4242 old KILL || status=$?
printf 'status=%s\n' "$status"
"""
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(marker.read_text(), "-TERM 4242")
        self.assertIn("status=1", result.stdout)

    def test_process_inspection_statuses_are_fail_closed(self) -> None:
        executable(self.bin / "ps", "exit 2")
        for pgrep_body, expected in (("exit 1", 0), ("exit 2", 1), ("echo 4242", 1)):
            with self.subTest(pgrep=pgrep_body):
                executable(self.bin / "pgrep", pgrep_body)
                result = self.shell(
                    """
APP_BUNDLE=/Applications/CuaDriver.app
LEGACY_APP_BUNDLE=/Applications/CuaDriverRs.app
HOME_DIR="$HOME/.cua-driver"
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
status=0; verify_release_daemon_absent || status=$?; exit "$status"
"""
                )
                self.assertEqual(result.returncode, expected, result.stderr)

    def test_supervisor_failure_preserves_runtime(self) -> None:
        tools = self.root / "tools"
        tools.mkdir()
        executable(tools / "id", "printf '501\\n'")
        executable(tools / "uname", "printf 'Linux\\n'")
        executable(tools / "systemctl", "exit 1")
        runtime = self.home / ".cua-driver/packages"
        runtime.mkdir(parents=True)
        unit = self.home / ".config/systemd/user/cua-driver.service"
        unit.parent.mkdir(parents=True)
        unit.write_text("[Service]\n")
        result = subprocess.run(
            ["/bin/bash", str(UNINSTALL)],
            cwd=ROOT,
            env={
                **self.env,
                "PATH": f"{tools}:/usr/bin:/bin",
                "CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY": "0",
            },
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertTrue(runtime.exists())
        self.assertTrue(unit.exists())


if __name__ == "__main__":
    unittest.main()
