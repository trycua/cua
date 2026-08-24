"""Tests for the release uninstaller's authoritative daemon shutdown path."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


def write_executable(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def source_helper(env: dict[str, str], body: str, *args: str) -> subprocess.CompletedProcess[str]:
    source = (
        'uninstall="$1"\n'
        'shift\n'
        'argv=("$@")\n'
        'source "$uninstall"\n'
        f"{body}\n"
    )
    return subprocess.run(
        ["/bin/bash", "-c", source, "bash", str(UNINSTALL), *args],
        cwd=REPO_ROOT,
        env={**env, "CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY": "1"},
        text=True,
        capture_output=True,
        check=False,
    )


class DaemonStopTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.fake_bin = self.root / "bin"
        self.fake_bin.mkdir()
        self.process_map = self.root / "process-map"
        self.process_map.touch()
        write_executable(
            self.fake_bin / "pgrep",
            f"""
while IFS='|' read -r pid identity state; do
    [ "$state" = alive ] || continue
    printf '%s\n' "$pid"
done < '{self.process_map}'
""",
        )
        write_executable(
            self.fake_bin / "ps",
            f"""
pid=""
previous=""
for argument in "$@"; do
    if [ "$previous" = "-p" ]; then pid="$argument"; fi
    previous="$argument"
done
case "$*" in
    *state=*)
        state=$(awk -F'|' -v pid="$pid" '$1 == pid {{ print $3 }}' '{self.process_map}')
        [ "$state" = alive ] && printf 'S\n' || printf 'Z\n'
        ;;
    *)
        identity=$(awk -F'|' -v pid="$pid" '$1 == pid {{ print $2 }}' '{self.process_map}')
        [ -z "$identity" ] || printf '%s serve\n' "$identity"
        ;;
esac
""",
        )
        self.env = os.environ.copy()
        self.env.update(
            {"HOME": str(self.home), "PATH": f"{self.fake_bin}:/usr/bin:/bin"}
        )
        self.processes: list[subprocess.Popen[bytes]] = []

    def tearDown(self) -> None:
        for process in self.processes:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
        self.temp_dir.cleanup()

    def compile_sleeper(self, destination: Path) -> Path:
        compiler = next(
            (shutil.which(name) for name in ("cc", "gcc", "clang") if shutil.which(name)),
            None,
        )
        if compiler is None:
            self.skipTest("no C compiler available")
        source = self.root / "sleeper.c"
        source.write_text("#include <unistd.h>\nint main(void){pause();return 0;}\n")
        destination.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [compiler, str(source), "-o", str(destination)],
            check=True,
            capture_output=True,
        )
        destination.chmod(0o755)
        return destination

    def spawn(self, executable: Path, *arguments: str, argv0: str | None = None) -> subprocess.Popen[bytes]:
        process = subprocess.Popen(
            [argv0 or str(executable), *arguments],
            executable=str(executable),
        )
        self.processes.append(process)
        with self.process_map.open("a", encoding="utf-8") as handle:
            handle.write(f"{process.pid}|{executable}|alive\n")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if process.poll() is None:
                return process
            time.sleep(0.01)
        raise AssertionError("fixture process did not start")

    def shell_setup(self, pid_file: Path, helper: Path | None, executable: Path) -> str:
        helper_value = str(helper) if helper else ""
        return f"""
OS=Linux
HOME_DIR="$HOME/.cua-driver"
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
APP_BUNDLE="/Applications/CuaDriver.app"
LEGACY_APP_BUNDLE="/Applications/CuaDriverRs.app"
PACKAGES_DIR="$HOME_DIR/packages"
DAEMON_PID_FILE="{pid_file}"
DAEMON_STOP_HELPER="{helper_value}"
DAEMON_EXECUTABLES=("{executable}")
"""

    def make_stop_helper(self, pid_file: Path, target_pid_file: Path | None = None) -> Path:
        helper = self.home / "cua-driver"
        log = self.root / "stop.log"
        target = target_pid_file or pid_file
        write_executable(
            helper,
            f"""
if [ "$1" = stop ]; then
    printf 'stop\n' >> '{log}'
    pid=$(cat '{target}')
    awk -F'|' -v pid="$pid" 'BEGIN {{ OFS="|" }} $1 == pid {{ $3="dead" }} {{ print }}' '{self.process_map}' > '{self.process_map}.tmp'
    mv '{self.process_map}.tmp' '{self.process_map}'
    kill "$pid" 2>/dev/null || true
    exit 0
fi
exit 64
""",
        )
        return helper

    def test_pid_file_path_matches_runtime_defaults(self) -> None:
        result = source_helper(
            self.env,
            'OS=Darwin; HOME=/users/test; daemon_pid_file_path',
        )
        self.assertEqual(
            result.stdout,
            "/users/test/Library/Caches/cua-driver/cua-driver.pid",
            result.stderr,
        )

        result = source_helper(
            self.env,
            'OS=Linux; HOME=/users/test; daemon_pid_file_path',
        )
        self.assertEqual(
            result.stdout,
            "/users/test/.cache/cua-driver/cua-driver.pid",
            result.stderr,
        )

    def test_stop_helper_resolves_cli_symlink_before_unlink(self) -> None:
        executable = self.compile_sleeper(
            self.home / ".cua-driver/packages/current/cua-driver"
        )
        link = self.home / ".local/bin/cua-driver"
        link.parent.mkdir(parents=True)
        link.symlink_to(executable)
        result = source_helper(
            self.env,
            f'''
OS=Linux
USER_BIN_LINK="{link}"
APP_BUNDLE="/Applications/CuaDriver.app"
LEGACY_APP_BUNDLE="/Applications/CuaDriverRs.app"
PACKAGES_DIR="$HOME/.cua-driver/packages"
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
select_daemon_stop_helper
''',
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, str(executable))

    def test_pid_file_and_stop_command_are_authoritative(self) -> None:
        executable = self.compile_sleeper(
            self.home / ".cua-driver/packages/current/cua-driver"
        )
        pid_file = self.home / "cua-driver.pid"
        process = self.spawn(executable, "serve")
        pid_file.write_text(str(process.pid), encoding="utf-8")
        helper = self.make_stop_helper(pid_file)

        result = source_helper(
            self.env,
            self.shell_setup(pid_file, helper, executable)
            + """
status=0
stop_release_daemon || status=$?
printf 'status=%s result=%s\n' "$status" "$DAEMON_STOP_RESULT"
""",
        )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("status=0 result=stopped", result.stdout)
        self.assertEqual((self.root / "stop.log").read_text(), "stop\n")
        process.wait(timeout=5)

    def test_pid_file_path_does_not_kill_an_mcp_sibling(self) -> None:
        executable = self.compile_sleeper(
            self.home / ".cua-driver/packages/current/cua-driver"
        )
        pid_file = self.home / "cua-driver.pid"
        daemon = self.spawn(executable, "serve")
        mcp = self.spawn(executable, "mcp")
        pid_file.write_text(str(daemon.pid), encoding="utf-8")
        helper = self.make_stop_helper(pid_file)

        result = source_helper(
            self.env,
            self.shell_setup(pid_file, helper, executable)
            + """
status=0
stop_release_daemon || status=$?
printf 'status=%s result=%s\n' "$status" "$DAEMON_STOP_RESULT"
""",
        )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("status=0 result=stopped", result.stdout)
        daemon.wait(timeout=5)
        self.assertIsNone(mcp.poll(), result.stdout + result.stderr)

    def test_stale_pid_uses_executable_identity_fallback(self) -> None:
        executable = self.compile_sleeper(
            self.home / ".cua-driver/packages/current/cua-driver"
        )
        pid_file = self.home / "cua-driver.pid"
        pid_file.write_text("2147483646\n", encoding="utf-8")
        process = self.spawn(executable, "serve")
        target_pid_file = self.home / "actual.pid"
        target_pid_file.write_text(str(process.pid), encoding="utf-8")
        helper = self.make_stop_helper(pid_file, target_pid_file)

        result = source_helper(
            self.env,
            self.shell_setup(pid_file, helper, executable)
            + """
status=0
stop_release_daemon || status=$?
printf 'status=%s result=%s\n' "$status" "$DAEMON_STOP_RESULT"
""",
        )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("status=0 result=stopped", result.stdout)
        process.wait(timeout=5)

    def test_foreign_executable_is_not_killed_by_fallback(self) -> None:
        installed = self.compile_sleeper(
            self.home / ".cua-driver/packages/current/cua-driver"
        )
        foreign = self.compile_sleeper(self.root / "vendor/cua-driver")
        process = self.spawn(foreign, "serve", argv0="cua-driver")
        pid_file = self.home / "cua-driver.pid"

        result = source_helper(
            self.env,
            self.shell_setup(pid_file, None, installed)
            + """
status=0
stop_release_daemon || status=$?
printf 'status=%s result=%s\n' "$status" "$DAEMON_STOP_RESULT"
""",
        )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("status=0 result=none", result.stdout)
        self.assertIsNone(process.poll(), result.stdout + result.stderr)

    def test_missing_process_tools_fail_closed(self) -> None:
        path = self.root / "empty-bin"
        path.mkdir()
        env = {**self.env, "PATH": str(path)}
        pid_file = self.home / "missing.pid"
        result = source_helper(
            env,
            self.shell_setup(pid_file, None, self.home / "cua-driver")
            + """
status=0
stop_release_daemon || status=$?
printf 'status=%s result=%s\n' "$status" "$DAEMON_STOP_RESULT"
""",
        )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("status=2 result=failed", result.stdout)
        self.assertIn("pgrep and ps are required", result.stderr)

    def test_main_marks_process_inspection_failure(self) -> None:
        path = self.root / "full-bin"
        path.mkdir()
        write_executable(path / "id", "printf '501\\n'")
        write_executable(path / "uname", "printf 'Linux\\n'")
        os.symlink("/bin/cat", path / "cat")
        os.symlink("/bin/rm", path / "rm")
        env = {**self.env, "PATH": str(path)}
        (self.home / ".cua-driver/packages").mkdir(parents=True)

        result = subprocess.run(
            ["/bin/bash", str(UNINSTALL)],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("daemon_stop_incomplete", result.stderr)
        self.assertIn("process inspection tools are unavailable", result.stdout)
        self.assertIn(
            "cua-driver uninstalled, but a cua-driver process is still running",
            result.stdout,
        )


if __name__ == "__main__":
    unittest.main()
