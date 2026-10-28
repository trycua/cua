# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Hermetic tests for the Linux launcher support: prompts, disk floor, pid ledger, evidence, pins."""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import json
import math
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))

import claude_arms as ca  # noqa: E402
import desktop_runtime as desktop  # noqa: E402
import run_bench as rb  # noqa: E402
from test_runner_flow import make_ctx  # noqa: E402


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name, BENCH / "tools" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


evidence = _load("check_run_evidence")
pins_tool = _load("make_linux_pins")

MACOS_PROMPT_SHA256 = "1f68c4cc0780099eb70e65d0e404c7e77bcdcc9909f622a7d1483f66e82bf06d"


class PlatformConstantsTest(unittest.TestCase):
    def test_prompts(self) -> None:
        self.assertEqual(
            hashlib.sha256(ca.MACOS_SYSTEM_PROMPT.encode()).hexdigest(), MACOS_PROMPT_SHA256
        )
        self.assertEqual(
            ca.LINUX_SYSTEM_PROMPT,
            ca.MACOS_SYSTEM_PROMPT.replace("a macOS desktop", "a Linux desktop"),
        )
        self.assertIn("a Linux desktop", ca.LINUX_SYSTEM_PROMPT)
        want = (
            ca.LINUX_SYSTEM_PROMPT if sys.platform.startswith("linux") else ca.MACOS_SYSTEM_PROMPT
        )
        self.assertEqual(ca.SYSTEM_PROMPT, want)

    def test_executable_and_path(self) -> None:
        if sys.platform == "darwin":
            self.assertEqual(ca.CLAUDE_BIN, Path("/opt/homebrew/bin/claude"))
            self.assertEqual(ca.CLAUDE_PATH, "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        else:
            self.assertTrue(ca.CLAUDE_PATH.startswith(f"{ca.CLAUDE_BIN.parent}:"))
            self.assertNotIn("homebrew", ca.CLAUDE_PATH)

    def test_env_allowlist_unchanged(self) -> None:
        self.assertEqual(ca.CLAUDE_ENV_ALLOW, ("HOME", "PATH", "USER", "LANG", "TMPDIR", "TERM"))


class LinuxSingleBuildTest(unittest.TestCase):
    def test_explicit_binary_keeps_the_run_steps_only_arms_without_run_script(self) -> None:
        # One explicit driver backs every arm on Linux; it ships run_script, which only the script arm may expose.
        with tempfile.TemporaryDirectory() as tmp:
            saved = dict(ca.CUA_BUILDS)
            saved_globals = (ca.PINS_FILE, ca.CUA_BIN, ca.CUA_SKILLS)
            try:
                args = rb.build_parser().parse_args(
                    ["preflight", "--bench-v2", "--cua-bin", "/bin/true", "--cua-skills", tmp,
                     "--pins", str(Path(tmp) / "p.json"), "--runs-dir", tmp,
                     "--arms", *ca.CUA_ARMS]
                )  # fmt: skip
                with mock.patch.object(rb.Ctx, "__init__", return_value=None):
                    rb.make_ctx(args)
                for arm, build in ca.CUA_BUILDS.items():
                    self.assertEqual(
                        ca.SCRIPT_FLAG in build.daemon_env, arm == "cc-cua-driver-script", arm
                    )
                    self.assertEqual(
                        ca.NO_SCRIPT_FLAG in build.daemon_env, arm != "cc-cua-driver-script", arm
                    )
            finally:
                ca.CUA_BUILDS.clear()
                ca.CUA_BUILDS.update(saved)
                ca.PINS_FILE, ca.CUA_BIN, ca.CUA_SKILLS = saved_globals


def _x11_ready() -> bool:
    if sys.platform != "linux" or not os.environ.get("DISPLAY"):
        return False
    try:
        return subprocess.run(["xdpyinfo"], capture_output=True, timeout=10).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def _benchlab_running() -> bool:
    return subprocess.run(["pgrep", "-f", r"^/usr/bin/python3 \S*linux/benchlab\.py"], capture_output=True).returncode == 0


@unittest.skipUnless(_x11_ready(), "headless X11 session required")
class LinuxPreflightCaptureTest(unittest.TestCase):
    """Preflight on a real desktop: the capture must be a valid, non-uniform frame, and nothing is left running."""

    def test_preflight_captures_a_window_and_cleans_up(self) -> None:
        arm = rb.cxn.MCP_ARMS[0]
        frames: list[str] = []

        def capture(ctx, name):
            # cursor-free, like the driver's own capture: an empty desktop must come out uniform
            shot = ctx.run_dir / "shot.png"
            subprocess.run(
                ["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "x11grab", "-draw_mouse", "0",
                 "-i", os.environ["DISPLAY"], "-frames:v", "1", str(shot)],
                check=True, timeout=30,
            )  # fmt: skip
            frames.append("running" if _benchlab_running() else "absent")
            try:  # the runner's own validity check: raises on a uniform frame
                return f"{name} real capture", "pass", rb.rec.capture_detail(shot)
            except RuntimeError as error:
                return f"{name} real capture", "fail", str(error)

        self.assertFalse(_benchlab_running())
        with tempfile.TemporaryDirectory() as tmp:
            ctx = make_ctx(tmp, offline=True, allow_non_terminal=True)
            ctx.task_ids = []
            ctx.arm_names = [arm]
            ctx.mcp[arm] = (Path(tmp) / "mcp.json", rb.cxn.SERVER_OF[arm])
            cli = Path(tmp) / "codex"
            cli.touch()
            with (
                mock.patch.object(rb, "terminal_ancestor", return_value=True),
                mock.patch.object(rb.rec, "free_gb", return_value=1000),
                mock.patch.object(rb, "check_pins", return_value=[]),
                mock.patch.object(rb, "registered_bench_copies", return_value=[]),
                mock.patch.object(rb.cxn, "CODEX_CLI", cli),
                mock.patch.object(
                    rb.cxn, "server_entry", return_value={"command": "mcp", "args": [], "env": {}}
                ),
                mock.patch.object(rb.ca, "cu_helper_observed", return_value={}),
                mock.patch.object(rb.ca, "check_cu_helper_pins", return_value=[]),
                mock.patch.object(rb, "cu_helper_live_checks", return_value=[]),
                mock.patch.object(rb, "native_codex_checks", return_value=[]),
                mock.patch.object(rb, "real_capture_check", side_effect=capture),
            ):
                rows = rb.preflight(ctx)
        by_name = {name: (status, detail) for name, status, detail in rows}
        status, detail = by_name[f"{arm} real capture"]
        self.assertEqual(status, "pass", detail)
        self.assertEqual(frames, ["running"])
        self.assertFalse(_benchlab_running())


class LinuxCaptureSubjectOffLinuxTest(unittest.TestCase):
    def test_not_used_on_macos(self) -> None:
        with (
            mock.patch.object(rb.desktop, "detect_platform", return_value="macos"),
            mock.patch.object(rb.desktop, "start_app", side_effect=AssertionError),
            tempfile.TemporaryDirectory() as tmp,
        ):
            with rb.capture_subject(Path(tmp)):
                pass


class DiskFloorTest(unittest.TestCase):
    def test_flag_lowers_the_floor(self) -> None:
        with (
            tempfile.TemporaryDirectory() as tmp,
            mock.patch.object(rb.rec, "free_gb", return_value=10.0),
        ):
            ctx = make_ctx(tmp, disk_floor_gb=5.0, no_quota=True)
            ctx.quota = {}
            with mock.patch.object(rb.core, "quota_gate", return_value={"action": "go"}):
                rb.check_gates(ctx)
            ctx.args.disk_floor_gb = rb.DISK_FLOOR_GB
            with self.assertRaises(rb.StopRun) as caught:
                rb.check_gates(ctx)
            self.assertEqual(caught.exception.reason, "DISK")

    def test_invalid_values_rejected(self) -> None:
        for bad in ("0", "-1", "nan", "inf"):
            with self.subTest(bad), self.assertRaises(SystemExit):
                with mock.patch("sys.stderr"):
                    rb.build_parser().parse_args(["preflight", "--disk-floor-gb", bad])
        self.assertTrue(math.isfinite(rb.positive_gb("5")))
        args = rb.build_parser().parse_args(["preflight"])
        self.assertEqual(args.disk_floor_gb, rb.DISK_FLOOR_GB)


class RecordPidTest(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_start_app_records_pid_without_argv(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            with mock.patch.dict(os.environ, {"CDB_BENCH_PID_LOG": str(ledger)}):
                proc = desktop.start_app(["sleep", "30"], Path(tmp) / "home", Path(tmp) / "log")
            try:
                entry = json.loads(ledger.read_text())
                self.assertEqual(entry["pid"], proc.pid)
                self.assertEqual(entry["pgid"], proc.pid)
                self.assertEqual(set(entry), {"pid", "pgid", "role", "start"})
                self.assertEqual(entry["role"], "sleep")
            finally:
                desktop.stop_group(proc.pid)
                proc.wait(5)

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_ledger_write_failure_reaps_the_new_process(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            made: list[subprocess.Popen] = []
            real = subprocess.Popen

            def spy(*a, **k):
                made.append(real(*a, **k))
                return made[-1]

            env = {"CDB_BENCH_PID_LOG": str(Path(tmp) / "missing" / "pids.jsonl")}
            with (
                mock.patch.dict(os.environ, env),
                mock.patch.object(desktop.subprocess, "Popen", spy),
                self.assertRaises(OSError),
            ):
                desktop.start_app(["sleep", "30"], Path(tmp) / "home", Path(tmp) / "log")
            self.assertEqual(len(made), 1)
            self.assertIsNotNone(made[0].poll())

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_sigterm_reaps_private_daemon(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile = Path(tmp) / "pid"
            fake = Path(tmp) / "driver"
            fake.write_text(
                f'#!/bin/sh\n[ "$1" = serve ] && {{ echo $$ > {pidfile}; exec sleep 300; }}\n'
                "exit 1\n"
            )
            fake.chmod(0o755)
            proc = subprocess.Popen(
                [sys.executable, str(BENCH / "desktop_runtime.py"), "doctor",
                 "--cua-bin", str(fake), "--out", str(Path(tmp) / "out")]
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 10
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.1)
                time.sleep(0.5)
                daemon = int(pidfile.read_text())
                proc.send_signal(signal.SIGTERM)
                proc.wait(20)
                time.sleep(0.5)
                with self.assertRaises(ProcessLookupError):
                    os.kill(daemon, 0)
            finally:
                if proc.poll() is None:
                    proc.kill()

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_sigterm_at_startup_handoff_reaps_daemon(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile = Path(tmp) / "pid"
            fake = Path(tmp) / "driver"
            fake.write_text(
                f'#!/bin/sh\n[ "$1" = serve ] && {{ echo $$ > {pidfile}; exec sleep 300; }}\n'
                "exit 1\n"
            )
            fake.chmod(0o755)
            real = desktop.start_app

            def start_then_term(*a, **k):
                proc = real(*a, **k)
                os.kill(os.getpid(), signal.SIGTERM)  # lands before the caller owns proc
                return proc

            with mock.patch.object(desktop, "start_app", start_then_term), self.assertRaises(
                SystemExit
            ) as raised:
                desktop.main(["doctor", "--cua-bin", str(fake), "--out", str(Path(tmp) / "out")])
            self.assertEqual(raised.exception.code, 143)
            daemon = int(pidfile.read_text())
            time.sleep(0.5)
            with self.assertRaises(ProcessLookupError):
                os.kill(daemon, 0)

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_cancel_then_retry_starts_cleanly(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile = Path(tmp) / "pid"
            fake = Path(tmp) / "driver"
            fake.write_text(
                f'#!/bin/sh\n[ "$1" = serve ] && {{ echo $$ > {pidfile}; exec sleep 300; }}\n'
                "exit 0\n"
            )
            fake.chmod(0o755)
            real = desktop.start_app

            def start_then_term(*a, **k):
                proc = real(*a, **k)
                os.kill(os.getpid(), signal.SIGTERM)
                return proc

            handler = signal.getsignal(signal.SIGTERM)
            with mock.patch.object(desktop, "start_app", start_then_term), self.assertRaises(
                SystemExit
            ) as raised:
                desktop.main(["doctor", "--cua-bin", str(fake), "--out", str(Path(tmp) / "o1")])
            self.assertEqual(raised.exception.code, 143)
            self.assertIs(signal.getsignal(signal.SIGTERM), handler)

            state = Path(tmp) / "state"
            (state / "home").mkdir(parents=True)
            proc = desktop.start_driver(fake, str(state / "d.sock"), state, overlay=False)
            try:
                self.assertIsNone(proc.poll())
            finally:
                desktop.stop_group(proc.pid)
                proc.wait(5)

            with mock.patch("sys.stdout"):
                code = desktop.main(["doctor", "--cua-bin", str(fake), "--out", str(Path(tmp) / "o2")])
            self.assertIn(code, (0, 1))
            report = json.loads((Path(tmp) / "o2" / "doctor.json").read_text())
            self.assertEqual(report["daemon"], "private")
            time.sleep(0.5)
            with self.assertRaises(ProcessLookupError):
                os.kill(report["daemon_pid"], 0)

    def test_cancel_helpers_are_inert_outside_doctor(self) -> None:
        with desktop.deferred_cancel():
            desktop.check_cancel()
        desktop.check_cancel()

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_doctor_second_sigterm_during_teardown_still_reaps_daemon(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pidfile, termed = Path(tmp) / "pid", Path(tmp) / "termed"
            fake = Path(tmp) / "driver"
            fake.write_text(
                "#!/bin/sh\n"
                f'[ "$1" = serve ] && {{ echo $$ > {pidfile}; '
                f"trap 'touch {termed}' TERM; while :; do sleep 0.1; done; }}\n"
                "exit 1\n"
            )
            fake.chmod(0o755)
            proc = subprocess.Popen(
                [sys.executable, str(BENCH / "desktop_runtime.py"), "doctor",
                 "--cua-bin", str(fake), "--out", str(Path(tmp) / "out")]
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 10
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.1)
                time.sleep(0.5)
                daemon = int(pidfile.read_text())
                proc.send_signal(signal.SIGTERM)
                deadline = time.monotonic() + 10
                while not termed.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                proc.send_signal(signal.SIGTERM)
                proc.wait(20)
                time.sleep(0.5)
                with self.assertRaises(ProcessLookupError):
                    os.kill(daemon, 0)
            finally:
                if proc.poll() is None:
                    proc.kill()

    def test_noop_without_variable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ):
            os.environ.pop("CDB_BENCH_PID_LOG", None)
            self.assertIsNone(desktop.record_pid(os.getpid(), "x"))

    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_cdb_start_apps_records_before_readiness_failure(self) -> None:
        import cdb_adapter

        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            task = cdb_adapter.CdbTask.__new__(cdb_adapter.CdbTask)
            task.artifacts = Path(tmp)
            task.bundle = Path(tmp)
            task.procs, task.log = [], []
            task.sub = lambda text, **_: text
            app = {
                "id": "stub",
                "command": ["sleep", "30"],
                "ready": {"http": "http://127.0.0.1:1/", "timeout_seconds": 0.2},
            }
            task.apps = lambda: [app]
            try:
                with mock.patch.dict(os.environ, {"CDB_BENCH_PID_LOG": str(ledger)}):
                    with self.assertRaises(RuntimeError):
                        task.start_apps()
                self.assertEqual(json.loads(ledger.read_text())["role"], "stub")
            finally:
                for proc in task.procs:
                    desktop.stop_group(proc.pid)
                    proc.wait(5)


def write_run(root: Path, *, arm: str = "cc-cua-driver", recorder: str = "none", argv=None) -> Path:
    run = root / "run"
    attempt = run / "trials" / "T1" / "a1"
    (attempt / "artifacts").mkdir(parents=True)
    (attempt / "trial.json").write_text("{}")
    (attempt / "artifacts" / "failure.png").write_bytes(b"png")
    (attempt / "artifacts" / "driver.log").write_text("log")
    (attempt / "artifacts" / "sentinel.jsonl").write_text('{"event": 1}\n')
    row = {"trial_id": "T1", "attempt": 1, "arm": arm, "passed": False, "recording_started": False}
    (run / "results.jsonl").write_text(json.dumps(row) + "\n")
    (run / "manifest.json").write_text(json.dumps({"recorder": recorder, "argv": argv or []}))
    return run


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux doctor")
class SpawnOwnershipTest(unittest.TestCase):
    """An unwritable ledger must fail the launch and leave no live child."""

    def setUp(self) -> None:
        if sys.platform != "linux":
            self.skipTest("Linux only")
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.made: list[subprocess.Popen] = []
        real = subprocess.Popen

        def spy(*a, **k):
            self.made.append(real(*a, **k))
            return self.made[-1]

        self.spy = spy
        self.addCleanup(self._tmp.cleanup)
        self.addCleanup(self._reap)

    def _reap(self) -> None:
        for child in self.made:
            if child.poll() is None:
                with contextlib.suppress(OSError):
                    os.killpg(child.pid, signal.SIGKILL)
            with contextlib.suppress(Exception):
                child.wait(5)

    def ledger(self, writable: bool = False) -> dict[str, str]:
        if writable:
            return {"CDB_BENCH_PID_LOG": str(self.tmp / "pids.jsonl")}
        return {"CDB_BENCH_PID_LOG": str(self.tmp / "missing" / "pids.jsonl")}

    def check(self, count: int = 1) -> None:
        self.assertEqual(len(self.made), count)
        for child in self.made:
            self.assertIsNotNone(child.poll())

    def test_agent_launch(self) -> None:
        import claude_driver

        with (
            mock.patch.dict(os.environ, self.ledger()),
            mock.patch.object(claude_driver.ca, "open_token_fd", return_value=None),
            mock.patch.object(claude_driver.subprocess, "Popen", self.spy),
            self.assertRaises(OSError),
        ):
            claude_driver.run_claude(
                ["sleep", "30"],
                dict(os.environ),
                self.tmp,
                "p",
                set(),
                5.0,
                self.tmp / "s",
                self.tmp / "e",
                self.tmp / "l",
            )
        self.check()

    def test_app_server_launch(self) -> None:
        import codex_native

        with (
            mock.patch.dict(os.environ, self.ledger()),
            mock.patch.object(codex_native.subprocess, "Popen", self.spy),
            self.assertRaises(OSError),
        ):
            codex_native.run_codex(
                ["sleep", "30"],
                dict(os.environ),
                self.tmp,
                "p",
                5.0,
                1,
                self.tmp / "r",
                self.tmp / "s",
                self.tmp / "e",
            )
        self.check()

    def test_pilot_run_agent(self) -> None:
        import run_pilot

        with (
            mock.patch.dict(os.environ, self.ledger()),
            mock.patch.object(run_pilot.arms, "render_home", return_value=dict(os.environ)),
            mock.patch.object(run_pilot.arms, "codex_argv", return_value=["sleep", "30"]),
            mock.patch.object(run_pilot.subprocess, "Popen", self.spy),
            self.assertRaises(OSError),
        ):
            (self.tmp / "t").mkdir()
            run_pilot.run_agent("a", "p", self.tmp, self.tmp / "t", 5, "m", "low")
        self.check()

    def launch_apps(self, apps: list[dict]) -> None:
        import run_pilot

        compare = mock.Mock()
        compare.expand_placeholders = lambda raw, variables: raw
        compare._resolve_executable = lambda command: command
        with mock.patch.object(run_pilot.subprocess, "Popen", self.spy):
            run_pilot.launch_apps(
                compare,
                {"apps": apps},
                {"bundle": str(self.tmp)},
                dict(os.environ),
                self.tmp / "logs",
            )

    def test_launch_apps_first_registration_fails(self) -> None:
        with mock.patch.dict(os.environ, self.ledger()), self.assertRaises(OSError):
            self.launch_apps([{"id": "a", "command": ["sleep", "30"]}])
        self.check()

    def test_launch_apps_later_registration_fails(self) -> None:
        real = desktop.record_pid
        calls = [0]

        def once(pid: int, role: str) -> None:
            calls[0] += 1
            if calls[0] > 1:
                raise OSError("ledger")
            real(pid, role)

        apps = [{"id": "a", "command": ["sleep", "30"]}, {"id": "b", "command": ["sleep", "30"]}]
        with (
            mock.patch.dict(os.environ, self.ledger(True)),
            mock.patch.object(desktop, "record_pid", once),
            self.assertRaises(OSError),
        ):
            self.launch_apps(apps)
        self.check(2)

    def test_launch_apps_readiness_fails(self) -> None:
        apps = [
            {"id": "a", "command": ["sleep", "30"]},
            {
                "id": "b",
                "command": ["sleep", "30"],
                "ready": {"http": "http://127.0.0.1:9/", "timeout_seconds": 1},
            },
        ]
        with mock.patch.dict(os.environ, self.ledger(True)), self.assertRaises(TimeoutError):
            self.launch_apps(apps)
        self.check(2)

    def test_record_or_reap_disabled_and_enabled(self) -> None:
        proc = subprocess.Popen(["sleep", "30"], start_new_session=True)
        self.made.append(proc)
        with mock.patch.dict(os.environ, self.ledger(True)):
            desktop.record_or_reap(proc, "sleep")
        self.assertIsNone(proc.poll())
        self.assertEqual(len((self.tmp / "pids.jsonl").read_text().splitlines()), 1)
        env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
        with mock.patch.dict(os.environ, env, clear=True):
            desktop.record_or_reap(proc, "sleep")
        self.assertEqual(len((self.tmp / "pids.jsonl").read_text().splitlines()), 1)

    def test_record_or_reap_reraises_same_exception_and_reaps_group(self) -> None:
        proc = subprocess.Popen(["sh", "-c", "sleep 30 & wait"], start_new_session=True)
        self.made.append(proc)
        time.sleep(0.3)
        boom = KeyboardInterrupt()
        with (
            mock.patch.object(desktop, "record_pid", side_effect=boom),
            self.assertRaises(KeyboardInterrupt) as caught,
        ):
            desktop.record_or_reap(proc, "x")
        self.assertIs(caught.exception, boom)
        self.assertIsNotNone(proc.poll())
        with self.assertRaises(ProcessLookupError):
            os.killpg(proc.pid, 0)


class DoctorStartupLogTest(unittest.TestCase):
    def test_failed_start_keeps_driver_log(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "driver"
            fake.write_text("#!/bin/sh\necho startup-boom >&2\nexit 3\n")
            fake.chmod(0o755)
            out = Path(tmp) / "out"
            self.assertEqual(desktop.main(["doctor", "--cua-bin", str(fake), "--out", str(out)]), 1)
            report = json.loads((out / "doctor.json").read_text())
            self.assertTrue(any(c["name"] == "driver-start" for c in report["checks"]))
            self.assertIn("startup-boom", (out / "driver.log").read_text())


class LedgerReuseTest(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux", "Linux only")
    def test_recycled_pid_is_not_the_recorded_child(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            stale = {"pid": os.getpid(), "pgid": os.getpid(), "role": "old", "start": 1}
            ledger.write_text(json.dumps(stale) + "\n")
            self.assertEqual(evidence.check_ledger(ledger), [])
            live = dict(stale, start=desktop.proc_start(os.getpid()))
            ledger.write_text(json.dumps(live) + "\n")
            self.assertTrue(any("still alive" in p for p in evidence.check_ledger(ledger)))


class EvidenceTest(unittest.TestCase):
    def check(self, run: Path) -> list[str]:
        return evidence.check_run(run)

    def test_complete_failed_task_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(self.check(write_run(Path(tmp))), [])

    def test_each_gap_is_named(self) -> None:
        cases = {
            "screenshot": lambda r: (r / "trials/T1/a1/artifacts/failure.png").unlink(),
            "driver.log": lambda r: (r / "trials/T1/a1/artifacts/driver.log").unlink(),
            "sentinel": lambda r: (r / "trials/T1/a1/artifacts/sentinel.jsonl").write_text(""),
            "orphan": lambda r: (r / "trials/T2/a1").mkdir(parents=True),
            "duplicate": lambda r: (r / "results.jsonl").write_text(
                (r / "results.jsonl").read_text() * 2
            ),
            "malformed": lambda r: (r / "results.jsonl").write_text("{nope\n"),
        }
        for word, mutate in cases.items():
            with self.subTest(word), tempfile.TemporaryDirectory() as tmp:
                run = write_run(Path(tmp))
                mutate(run)
                self.assertTrue(any(word in p for p in self.check(run)), word)

    def test_recorder_requires_started_and_video(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = write_run(Path(tmp), recorder="cua")
            self.assertTrue(any("recording not started" in p for p in self.check(run)))
            row = json.loads((run / "results.jsonl").read_text())
            row["recording_started"] = True
            (run / "results.jsonl").write_text(json.dumps(row) + "\n")
            self.assertTrue(any("video missing" in p for p in self.check(run)))
            video = run / "trials/T1/a1/video"
            video.mkdir()
            (video / "video.mp4").write_bytes(b"x")
            self.assertTrue(any("video missing" in p for p in self.check(run)))
            (video / "video-720p.mp4").write_bytes(b"x")
            (video / "video.json").write_text(json.dumps({"error": "OSError: boom"}))
            self.assertTrue(any("compression failed" in p for p in self.check(run)))
            (video / "video.json").write_text("{}")
            self.assertEqual(self.check(run), [])

    def test_bad_json_shapes_are_reported(self) -> None:
        for row in ({"trial_id": [], "attempt": 1, "arm": "x"}, {"trial_id": "T1", "attempt": 1}):
            with tempfile.TemporaryDirectory() as tmp:
                run = write_run(Path(tmp))
                (run / "results.jsonl").write_text(json.dumps(row) + "\n")
                self.assertTrue(any("malformed row" in p for p in self.check(run)))
        with tempfile.TemporaryDirectory() as tmp:
            run = write_run(Path(tmp))
            (run / "manifest.json").write_text("[]")
            self.assertTrue(any("manifest.json" in p for p in self.check(run)))
            self.assertEqual(evidence.main([str(run)]), 1)
            self.assertTrue((run / "evidence-check.json").is_file())

    def test_session_layout_ledger_is_found(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            session = Path(tmp)
            run = write_run(session / "runs")
            run = run.rename(session / "runs" / "RUN")
            live = subprocess.Popen(["sleep", "30"])
            try:
                (session / "pids.jsonl").write_text(
                    json.dumps({"pid": live.pid, "pgid": None, "role": "s"}) + "\n"
                )
                self.assertTrue(any("still alive" in p for p in self.check(run)))
            finally:
                live.kill()
                live.wait()

    @unittest.skipUnless(os.path.isdir("/proc/self"), "needs /proc")
    def test_zombie_leader_with_live_child_is_alive(self) -> None:
        leader = subprocess.Popen(
            ["bash", "-c", "sleep 30 & echo $!; exit 0"],
            stdout=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        child = int(leader.stdout.readline())
        try:
            time.sleep(0.3)  # leader exits, stays a zombie until waited
            with tempfile.TemporaryDirectory() as tmp:
                ledger = Path(tmp) / "pids.jsonl"
                ledger.write_text(
                    json.dumps({"pid": leader.pid, "pgid": leader.pid, "role": "l"}) + "\n"
                )
                self.assertTrue(
                    any("group still alive" in p for p in evidence.check_ledger(ledger))
                )
        finally:
            os.kill(child, signal.SIGKILL)
            leader.wait()

    def test_opt_outs_lift_requirements(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = write_run(Path(tmp), argv=["--no-sentinel"])
            (run / "trials/T1/a1/artifacts/sentinel.jsonl").unlink()
            self.assertEqual(self.check(run), [])

    def test_ledger(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "pids.jsonl"
            done = subprocess.Popen(["true"])
            done.wait()
            ledger.write_text(json.dumps({"pid": done.pid, "pgid": None, "role": "x"}) + "\n")
            self.assertEqual(evidence.check_ledger(ledger), [])
            live = subprocess.Popen(["sleep", "30"], start_new_session=True)
            try:
                ledger.write_text(
                    json.dumps({"pid": live.pid, "pgid": live.pid, "role": "s"}) + "\n"
                )
                self.assertTrue(evidence.check_ledger(ledger))
                self.assertIsNone(live.poll())
                with mock.patch.object(evidence.os, "kill", side_effect=PermissionError):
                    self.assertTrue(evidence.check_ledger(ledger))
                seen = []
                with (
                    mock.patch.object(evidence.os, "kill", lambda p, s: seen.append(s)),
                    mock.patch.object(evidence.os, "killpg", lambda p, s: seen.append(s)),
                ):
                    evidence.check_ledger(ledger)
                self.assertEqual(set(seen), {0})
            finally:
                os.killpg(live.pid, signal.SIGKILL)
                live.wait()
            ledger.write_text("not json\n")
            self.assertTrue(any("malformed" in p for p in evidence.check_ledger(ledger)))
            ledger.write_text('{"pid": 0}\n')
            self.assertTrue(evidence.check_ledger(ledger))


class PinsTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.bin = root / "cua-driver"
        self.bin.write_text('#!/bin/sh\necho "cua-driver 1.2.3"\n')
        self.bin.chmod(0o755)
        self.skills = root / "skills"
        self.skills.mkdir()
        (self.skills / "SKILL.md").write_text("skill")
        self.report = {
            "ok": True,
            "build": {
                "version": "1.2.3",
                "git_sha": "abc123",
                "exe_sha256": ca.sha256_file(self.bin),
            },
        }
        self.doctor = root / "doctor.json"
        self.doctor.write_text(json.dumps(self.report))
        self.out = root / "pins" / "linux.json"

    def run_tool(self) -> int:
        return pins_tool.main(
            [
                "--cua-bin",
                str(self.bin),
                "--cua-skills",
                str(self.skills),
                "--doctor-json",
                str(self.doctor),
                "--out",
                str(self.out),
            ]  # fmt: skip
        )

    def test_written_pins_satisfy_the_runner(self) -> None:
        with mock.patch.object(pins_tool, "in_checkout", return_value=False):
            self.assertEqual(self.run_tool(), 0)
        pins = json.loads(self.out.read_text())
        digest = ca.sha256_file(self.bin)
        self.assertEqual(pins["platform"], "linux")
        main = pins["cua_main"]
        self.assertEqual({main["binary_sha256"], main["script_binary_sha256"]}, {digest})
        self.assertEqual(pins["cua_skills_tree_sha256"], ca.sha256_tree(self.skills))
        checked_in = json.loads((BENCH / "pins.json").read_text())
        self.assertEqual(pins["cdb_pack"], checked_in["cdb_pack"])
        health = {
            "build": self.report["build"],
            "checks": {"ax_capability": "pass", "screen_capture_capability": "pass"},
            "ok": True,
        }
        with (
            mock.patch.object(rb.ca, "cua_health", return_value=health),
            mock.patch.object(rb.desktop, "capabilities_ok", return_value=True),
        ):
            rows = rb.check_daemon(pins, "cc-cua-driver-script")
        self.assertEqual([r[1] for r in rows], ["pass", "pass", "pass"])

    def test_refusals(self) -> None:
        def refuses(mutate) -> None:
            mutate()
            with mock.patch.object(pins_tool, "in_checkout", return_value=False):
                with self.assertRaises(SystemExit):
                    self.run_tool()

        refuses(lambda: self.report["build"].update(exe_sha256="0" * 64) or self.write())
        self.report["build"]["exe_sha256"] = ca.sha256_file(self.bin)
        refuses(lambda: self.report["build"].update(version="9.9") or self.write())
        self.report["build"]["version"] = "1.2.3"
        refuses(lambda: self.report["build"].update(git_sha="") or self.write())
        self.report["build"]["git_sha"] = "abc123"
        refuses(lambda: (self.skills / "SKILL.md").unlink())
        (self.skills / "SKILL.md").write_text("skill")
        self.out.parent.mkdir(parents=True)
        self.out.write_text("old")
        refuses(lambda: None)
        self.assertEqual(self.out.read_text(), "old")

    def test_competing_writer_is_preserved(self) -> None:
        self.out.parent.mkdir(parents=True)
        real_link = os.link

        def racing(src, dst):
            self.out.write_text("competitor")
            return real_link(src, dst)

        with (
            mock.patch.object(pins_tool, "in_checkout", return_value=False),
            mock.patch.object(pins_tool.os, "link", racing),
            self.assertRaises(SystemExit),
        ):
            self.run_tool()
        self.assertEqual(self.out.read_text(), "competitor")
        self.assertEqual([p.name for p in self.out.parent.iterdir()], [self.out.name])

    def test_refuses_inside_checkout(self) -> None:
        self.out = BENCH / "should-not-exist.json"
        with self.assertRaises(SystemExit):
            self.run_tool()
        self.assertFalse(self.out.exists())

    def write(self) -> None:
        self.doctor.write_text(json.dumps(self.report))


if __name__ == "__main__":
    unittest.main()
