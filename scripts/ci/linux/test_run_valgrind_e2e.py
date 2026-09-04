import importlib.util
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

RUNNER_PATH = Path(__file__).with_name("run-valgrind-e2e.py")
SPEC = importlib.util.spec_from_file_location("run_valgrind_e2e", RUNNER_PATH)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)

ShutdownTimeouts = RUNNER.ShutdownTimeouts
resolve_exit_status = RUNNER.resolve_exit_status
shutdown_server = RUNNER.shutdown_server


HELPER_TIMEOUTS = ShutdownTimeouts(
    client_stop=0.2,
    graceful_wait=0.2,
    sigterm_wait=0.4,
    sigkill_wait=0.4,
    poll_interval=0.01,
)


class ShutdownServerTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.client_log = self.root / "client.log"
        self.processes = []

    def tearDown(self):
        for process in self.processes:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=1)
        self.temp_dir.cleanup()

    def start_helper(self, source):
        ready = self.root / f"ready-{len(self.processes)}"
        process = subprocess.Popen(
            [sys.executable, "-c", source, str(ready)],
            start_new_session=True,
        )
        self.processes.append(process)
        deadline = time.monotonic() + 2
        while not ready.exists():
            if process.poll() is not None:
                self.fail(f"helper exited early with {process.returncode}")
            if time.monotonic() >= deadline:
                self.fail("helper did not become ready")
            time.sleep(0.01)
        return process

    def test_normal_client_stop_needs_no_signal(self):
        stop_marker = self.root / "stop"
        process = self.start_helper(
            "import pathlib, sys, time\n"
            "ready = pathlib.Path(sys.argv[1])\n"
            f"stop = pathlib.Path({str(stop_marker)!r})\n"
            "ready.touch()\n"
            "while not stop.exists(): time.sleep(0.01)\n"
        )
        stop_command = [
            sys.executable,
            "-c",
            "import pathlib, sys; pathlib.Path(sys.argv[1]).touch()",
            str(stop_marker),
        ]

        result = shutdown_server(process, stop_command, self.client_log, HELPER_TIMEOUTS)

        self.assertEqual(result.escalation, "none")
        self.assertEqual(result.returncode, 0)
        self.assertFalse(result.forced_cleanup)
        self.assertTrue(result.stop_attempted)

    def test_already_exited_server_skips_stop_and_reports_it(self):
        process = subprocess.Popen(
            [sys.executable, "-c", "import sys; sys.exit(3)"],
            start_new_session=True,
        )
        self.processes.append(process)
        process.wait(timeout=2)
        stop_marker = self.root / "stop"
        stop_command = [
            sys.executable,
            "-c",
            "import pathlib, sys; pathlib.Path(sys.argv[1]).touch()",
            str(stop_marker),
        ]

        result = shutdown_server(process, stop_command, self.client_log, HELPER_TIMEOUTS)

        self.assertFalse(result.stop_attempted)
        self.assertEqual(result.escalation, "none")
        self.assertEqual(result.returncode, 3)
        self.assertFalse(result.forced_cleanup)
        self.assertFalse(stop_marker.exists())

    def test_sigterm_escalation_is_bounded(self):
        process = self.start_helper(
            "import pathlib, signal, sys, time\n"
            "signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))\n"
            "pathlib.Path(sys.argv[1]).touch()\n"
            "while True: time.sleep(1)\n"
        )

        result = shutdown_server(
            process,
            [sys.executable, "-c", "pass"],
            self.client_log,
            HELPER_TIMEOUTS,
        )

        self.assertEqual(result.escalation, "sigterm")
        self.assertEqual(result.returncode, 0)
        self.assertTrue(result.forced_cleanup)
        self.assertTrue(result.stop_attempted)

    def test_sigkill_escalation_cannot_hang(self):
        process = self.start_helper(
            "import pathlib, signal, sys, time\n"
            "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
            "pathlib.Path(sys.argv[1]).touch()\n"
            "while True: time.sleep(1)\n"
        )
        started = time.monotonic()

        result = shutdown_server(
            process,
            [sys.executable, "-c", "pass"],
            self.client_log,
            HELPER_TIMEOUTS,
        )

        self.assertLess(time.monotonic() - started, 1.5)
        self.assertEqual(result.escalation, "sigkill")
        self.assertEqual(result.returncode, -signal.SIGKILL)
        self.assertTrue(result.forced_cleanup)
        self.assertTrue(result.stop_attempted)

    def test_status_precedence_preserves_primary_failure(self):
        cases = [
            (7, 99, True, 7),
            (0, 99, False, 99),
            (0, 99, True, 99),
            (0, -signal.SIGTERM, True, 1),
            (0, 0, True, 1),
            (0, -signal.SIGTERM, False, 128 + signal.SIGTERM),
            (0, 0, False, 0),
        ]
        for primary, server, forced, expected in cases:
            with self.subTest(primary=primary, server=server, forced=forced):
                self.assertEqual(
                    resolve_exit_status(primary, server, forced),
                    expected,
                )


if __name__ == "__main__":
    unittest.main()
