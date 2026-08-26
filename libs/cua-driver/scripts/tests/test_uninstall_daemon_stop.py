"""Focused coverage for fail-closed Unix release daemon shutdown."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import unittest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def executable(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def source(env: dict[str, str], body: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["/bin/bash", "-c", f'source "{UNINSTALL}"\n{body}'],
        cwd=REPO_ROOT,
        env={**env, "CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY": "1"},
        text=True,
        capture_output=True,
        check=False,
    )


class DaemonStopTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.bin = self.root / "bin"
        self.home.mkdir()
        self.bin.mkdir()
        self.map = self.root / "process-map"
        self.map.touch()

        executable(
            self.bin / "pgrep",
            f"awk -F'|' '$3 == \"alive\" {{ print $1 }}' '{self.map}'",
        )
        executable(
            self.bin / "ps",
            f"""
pid=""; prev=""
for arg in "$@"; do [ "$prev" = -p ] && pid="$arg"; prev="$arg"; done
awk -F'|' -v p="$pid" '$1 == p {{ print $2 }}' '{self.map}'
""",
        )
        self.env = {
            **os.environ,
            "HOME": str(self.home),
            "PATH": f"{self.bin}:/usr/bin:/bin",
        }
        self.processes: list[subprocess.Popen[bytes]] = []

    def tearDown(self) -> None:
        for process in self.processes:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=5)
        self.temp.cleanup()

    def spawn(self, command: str) -> tuple[subprocess.Popen[bytes], str]:
        path = shutil.which(command)
        if not path:
            self.skipTest(f"{command} unavailable")
        real = str(Path(path).resolve())
        args = [path, "30"] if command == "sleep" else [path, "-f", "/dev/null"]
        process = subprocess.Popen(args)
        self.processes.append(process)
        threading.Thread(target=process.wait, daemon=True).start()
        with self.map.open("a", encoding="utf-8") as handle:
            handle.write(f"{process.pid}|{real}|alive\n")
        return process, real

    def setup(self, pid_file: Path, helper: Path | None, identity: str) -> str:
        return f"""
OS=Linux
HOME_DIR="$HOME/.cua-driver"
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
DAEMON_PID_FILE="{pid_file}"
DAEMON_STOP_HELPER="{helper or ''}"
DAEMON_EXECUTABLES=("{identity}")
"""

    def helper(self, target: subprocess.Popen[bytes], marker: Path | None = None) -> Path:
        path = self.home / "cua-driver"
        marker_line = f"printf called > '{marker}'" if marker else ":"
        executable(
            path,
            f"""
if [ "$1" = stop ]; then
  {marker_line}
  awk -F'|' -v p='{target.pid}' 'BEGIN {{ OFS="|" }} $1 == p {{ $3="dead" }} {{ print }}' '{self.map}' > '{self.map}.tmp'
  mv '{self.map}.tmp' '{self.map}'
  kill {target.pid} 2>/dev/null || true
  exit 0
fi
exit 1
""",
        )
        return path

    def run_stop(self, setup: str) -> subprocess.CompletedProcess[str]:
        return source(
            self.env,
            setup
            + """
status=0
stop_release_daemon || status=$?
printf 'status=%s result=%s\n' "$status" "$DAEMON_STOP_RESULT"
""",
        )

    def test_pid_file_and_trusted_stop_are_authoritative(self) -> None:
        daemon, identity = self.spawn("sleep")
        foreign, _ = self.spawn("tail")
        pid_file = self.home / "cua-driver.pid"
        pid_file.write_text(str(daemon.pid), encoding="utf-8")

        result = self.run_stop(self.setup(pid_file, self.helper(daemon), identity))

        self.assertIn("status=0 result=stopped", result.stdout)
        daemon.wait(timeout=5)
        self.assertIsNone(foreign.poll())

    def test_stale_pid_only_inspects_and_never_calls_helper_or_signals(self) -> None:
        owned, identity = self.spawn("sleep")
        pid_file = self.home / "cua-driver.pid"
        pid_file.write_text("2147483646\n", encoding="utf-8")
        marker = self.root / "helper-called"

        result = self.run_stop(self.setup(pid_file, self.helper(owned, marker), identity))

        self.assertIn("status=1 result=failed", result.stdout)
        self.assertFalse(marker.exists())
        self.assertIsNone(owned.poll())

    def test_live_owned_pid_without_trusted_helper_fails_without_signal(self) -> None:
        daemon, identity = self.spawn("sleep")
        pid_file = self.home / "cua-driver.pid"
        pid_file.write_text(str(daemon.pid), encoding="utf-8")

        result = self.run_stop(self.setup(pid_file, None, identity))

        self.assertIn("status=1 result=failed", result.stdout)
        self.assertIn("trusted installed cua-driver stop helper is unavailable", result.stderr)
        self.assertIsNone(daemon.poll())

    def test_supervisor_failure_aborts_before_runtime_removal(self) -> None:
        tools = self.root / "main-bin"
        tools.mkdir()
        executable(tools / "id", "printf '501\\n'")
        executable(tools / "uname", "printf 'Linux\\n'")
        executable(tools / "systemctl", "exit 1")
        os.symlink("/bin/rm", tools / "rm")
        runtime = self.home / ".cua-driver/packages"
        runtime.mkdir(parents=True)
        unit = self.home / ".config/systemd/user/cua-driver.service"
        unit.parent.mkdir(parents=True)
        unit.write_text("[Service]\n", encoding="utf-8")

        result = subprocess.run(
            ["/bin/bash", str(UNINSTALL)],
            cwd=REPO_ROOT,
            env={**self.env, "PATH": f"{tools}:/usr/bin:/bin"},
            text=True,
            capture_output=True,
            check=False,
        )

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("failed to stop systemd user unit", result.stderr)
        self.assertTrue(runtime.exists())
        self.assertTrue(unit.exists())


if __name__ == "__main__":
    unittest.main()
