"""Focused coverage for Unix release daemon shutdown."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
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
case "$*" in
  *state=*) awk -F'|' -v p="$pid" '$1 == p && $3 == "alive" {{ print "S" }}' '{self.map}' ;;
  *) awk -F'|' -v p="$pid" '$1 == p {{ print $2 }}' '{self.map}' ;;
esac
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

    def stop_helper(self, target: subprocess.Popen[bytes]) -> Path:
        helper = self.home / "cua-driver"
        executable(
            helper,
            f"""
if [ "$1" = stop ]; then
  kill {target.pid} 2>/dev/null || true
  exit 0
fi
exit 1
""",
        )
        return helper

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

    def test_pid_file_paths_match_runtime_defaults(self) -> None:
        mac = source(self.env, 'OS=Darwin; HOME=/u; daemon_pid_file_path')
        linux = source(self.env, 'OS=Linux; HOME=/u; daemon_pid_file_path')
        self.assertEqual(mac.stdout, "/u/Library/Caches/cua-driver/cua-driver.pid")
        self.assertEqual(linux.stdout, "/u/.cache/cua-driver/cua-driver.pid")

    def test_pid_file_and_native_stop_are_authoritative_and_leave_mcp_sibling(self) -> None:
        daemon, identity = self.spawn("sleep")
        sibling, _ = self.spawn("sleep")
        pid_file = self.home / "cua-driver.pid"
        pid_file.write_text(str(daemon.pid), encoding="utf-8")

        result = self.run_stop(self.setup(pid_file, self.stop_helper(daemon), identity))

        self.assertIn("status=0 result=stopped", result.stdout)
        daemon.wait(timeout=5)
        self.assertIsNone(sibling.poll())

    def test_stale_pid_uses_native_stop_before_identity_fallback(self) -> None:
        daemon, identity = self.spawn("sleep")
        pid_file = self.home / "cua-driver.pid"
        pid_file.write_text("2147483646\n", encoding="utf-8")

        result = self.run_stop(self.setup(pid_file, self.stop_helper(daemon), identity))

        self.assertIn("status=0 result=stopped", result.stdout)
        daemon.wait(timeout=5)

    def test_identity_fallback_ignores_foreign_executable(self) -> None:
        _, installed = self.spawn("sleep")
        foreign, _ = self.spawn("tail")

        result = self.run_stop(self.setup(self.home / "missing.pid", None, installed))

        self.assertIn("status=1 result=failed", result.stdout)
        # The installed sleeper makes the fallback ambiguous and fail-closed;
        # the foreign executable must remain untouched as well.
        self.assertIsNone(foreign.poll())

    def test_identity_fallback_does_not_signal_ambiguous_owned_process(self) -> None:
        owned, identity = self.spawn("sleep")

        result = self.run_stop(self.setup(self.home / "missing.pid", None, identity))

        self.assertIn("status=1 result=failed", result.stdout)
        self.assertIn("cannot safely identify the daemon", result.stderr)
        self.assertIsNone(owned.poll())

    def test_missing_process_tools_fail_closed(self) -> None:
        empty = self.root / "empty"
        empty.mkdir()
        env = {**self.env, "PATH": str(empty)}
        result = source(
            env,
            self.setup(self.home / "missing.pid", None, "/missing/cua-driver")
            + """
status=0
stop_release_daemon || status=$?
printf 'status=%s result=%s\n' "$status" "$DAEMON_STOP_RESULT"
""",
        )
        self.assertIn("status=2 result=failed", result.stdout)
        self.assertIn("pgrep and ps are required", result.stderr)

    def test_main_propagates_process_inspection_failure(self) -> None:
        tools = self.root / "main-bin"
        tools.mkdir()
        executable(tools / "id", "printf '501\\n'")
        executable(tools / "uname", "printf 'Linux\\n'")
        os.symlink("/bin/rm", tools / "rm")
        (self.home / ".cua-driver/packages").mkdir(parents=True)

        result = subprocess.run(
            ["/bin/bash", str(UNINSTALL)],
            cwd=REPO_ROOT,
            env={**self.env, "PATH": str(tools)},
            text=True,
            capture_output=True,
            check=False,
        )

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("daemon_stop_incomplete", result.stderr)
        self.assertIn("process inspection tools are unavailable", result.stdout)


if __name__ == "__main__":
    unittest.main()
