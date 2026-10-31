# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Tests for the Linux session launcher (launch/linux_session.{sh,py})."""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))


def _x11_ready() -> bool:
    if sys.platform != "linux" or not os.environ.get("DISPLAY"):
        return False
    try:
        return (
            subprocess.run(
                ["xdpyinfo", "-display", os.environ["DISPLAY"]], capture_output=True, timeout=10
            ).returncode
            == 0
        )
    except (OSError, subprocess.SubprocessError):
        return False


def _has_gi() -> bool:
    try:
        import gi  # noqa: F401
    except ImportError:
        return False
    return True


def _has_session_tools() -> bool:
    import shutil

    return sys.platform == "linux" and all(
        shutil.which(t) for t in ("Xvfb", "dbus-daemon", "openbox", "picom", "xprop", "gdbus")
    )


def _has_xvfb_dbus() -> bool:
    import shutil

    return sys.platform == "linux" and all(shutil.which(t) for t in ("Xvfb", "dbus-daemon"))


FAKE_DRIVER = """#!/usr/bin/python3
import json, sys, time
a = sys.argv[1:]
if "--version" in a:
    print("cua-driver 9.9.9")
elif a and a[0] == "serve":
    time.sleep(600)
elif a and a[0] == "status":
    pass
elif "health_report" in a:
    print(json.dumps({"build": {"version": "9.9.9", "git_sha": "abc", "exe_sha256": "x"},
        "checks": [{"name": "ax_capability", "status": "pass"},
                   {"name": "screen_capture_capability", "status": "pass"}]}))
"""


@unittest.skipUnless(
    _has_session_tools(), "Xvfb, dbus-daemon, openbox, picom, xprop and gdbus required"
)
class LinuxLauncherTest(unittest.TestCase):
    script = HERE.parent / "launch" / "linux_session.sh"

    def launch(self, evidence: Path, *command: str) -> subprocess.CompletedProcess:
        env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
        return subprocess.run(
            ["bash", str(self.script), "--evidence", str(evidence), "--", *command],
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
        )

    def test_launcher_owns_and_reaps_its_session(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            done = self.launch(
                ev,
                "bash",
                "-c",
                'xprop -root _NET_SUPPORTING_WM_CHECK > "$CDB_SESSION_DIR/wm.txt"',
            )
            self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
            self.assertIn("window id", (ev / "wm.txt").read_text())
            address = (ev / "at-spi-address.txt").read_text()
            self.assertTrue("unix:path=" in address or "unix:abstract=" in address, address)
            entries = [json.loads(x) for x in (ev / "pids.jsonl").read_text().splitlines()]
            self.assertEqual(
                {e["role"] for e in entries}, {"xvfb", "dbus", "openbox", "picom", "command"}
            )
            for entry in entries:
                with self.assertRaises(ProcessLookupError):
                    os.kill(entry["pid"], 0)
            self.assertEqual(LinuxSessionSupervisorTest._session_pids(ev), [])
            self.assertEqual((ev / "exit").read_text().strip(), "command=0 evidence=0")

    def test_status_passes_through(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            done = self.launch(ev, "bash", "-c", "exit 7")
            self.assertEqual(done.returncode, 7)
            self.assertIn("command=7", (ev / "exit").read_text())
            self.assertTrue((ev / "launch.log").exists())

    def test_surviving_child_fails_the_check(self) -> None:
        def leaked_pids(ev: Path) -> list[int]:
            try:
                lines = (ev / "pids.jsonl").read_text().splitlines()
            except OSError:
                return []
            return [json.loads(x)["pid"] for x in lines if json.loads(x).get("role") == "leak"]

        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            try:
                done = self.launch(
                    ev,
                    "bash",
                    "-c",
                    'setsid sleep 300 & echo "{\\"pid\\": $!, \\"pgid\\": $!, \\"role\\": \\"leak\\"}"'
                    ' >> "$CDB_BENCH_PID_LOG"; sleep 0.5',
                )
                leaked = leaked_pids(ev)
                self.assertNotEqual(done.returncode, 0)
                self.assertEqual(len(leaked), 1)
                os.kill(leaked[0], 0)
                self.assertIn("command=0 evidence=1", (ev / "exit").read_text())
            finally:
                for pid in leaked_pids(ev):
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except OSError:
                        pass

    def test_unregistered_background_child_fails_the_check(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            done = self.launch(ev, "bash", "-c", 'sleep 300 & echo $! > "$CDB_SESSION_DIR/bg.pid"')
            try:
                self.assertEqual(done.returncode, 1, done.stderr + done.stdout)
                self.assertIn("command=0 evidence=1", (ev / "exit").read_text())
            finally:
                try:
                    os.kill(int((ev / "bg.pid").read_text()), signal.SIGKILL)
                except (OSError, ValueError):
                    pass

    def test_sigterm_waits_for_delayed_cleanup(self) -> None:
        script = (
            "import os,signal,subprocess,sys,time\n"
            "d=os.environ['CDB_SESSION_DIR']\n"
            "c=subprocess.Popen(['sleep','300'],start_new_session=True)\n"
            "open(d+'/child.pid','w').write(str(c.pid))\n"
            'open(d+\'/pids.jsonl\',\'a\').write(\'{"pid":%d,"pgid":%d,"role":"child"}\\n\'%(c.pid,c.pid))\n'
            "def h(*a):\n"
            "    time.sleep(2); c.kill(); c.wait(); open(d+'/cleaned','w').write('1'); sys.exit(143)\n"
            "signal.signal(signal.SIGTERM,h)\n"
            "open(d+'/ready','w').write('1')\n"
            "time.sleep(300)\n"
        )
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
            proc = subprocess.Popen(
                ["bash", str(self.script), "--evidence", str(ev), "--", sys.executable, "-c", script],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env,
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 60
                while not (ev / "ready").exists() and time.monotonic() < deadline:
                    time.sleep(0.2)
                self.assertTrue((ev / "ready").exists())
                proc.send_signal(signal.SIGTERM)
                self.assertNotEqual(proc.wait(60), 0)
                self.assertTrue((ev / "cleaned").exists())
                self.assertIn("command=143 evidence=0", (ev / "exit").read_text())
            finally:
                if proc.poll() is None:
                    proc.kill()
                try:
                    os.kill(int((ev / "child.pid").read_text()), signal.SIGKILL)
                except (OSError, ValueError):
                    pass

    def test_timeout_waits_for_delayed_cleanup(self) -> None:
        script = (
            "import os,signal,subprocess,sys,time\n"
            "d=os.environ['CDB_SESSION_DIR']\n"
            "c=subprocess.Popen(['sleep','300'],start_new_session=True)\n"
            "open(d+'/child.pid','w').write(str(c.pid))\n"
            'open(d+\'/pids.jsonl\',\'a\').write(\'{"pid":%d,"pgid":%d,"role":"child"}\\n\'%(c.pid,c.pid))\n'
            "def h(*a):\n"
            "    time.sleep(2); c.kill(); c.wait(); open(d+'/cleaned','w').write('1'); sys.exit(143)\n"
            "signal.signal(signal.SIGTERM,h)\n"
            "open(d+'/ready','w').write('1')\n"
            "time.sleep(300)\n"
        )
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
            env["CDB_SESSION_TIMEOUT"] = "8"
            proc = subprocess.Popen(
                ["bash", str(self.script), "--evidence", str(ev), "--", sys.executable, "-c", script],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env,
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 60
                while not (ev / "ready").exists() and time.monotonic() < deadline:
                    time.sleep(0.2)
                self.assertTrue((ev / "ready").exists())
                self.assertEqual(proc.wait(90), 124)
                self.assertTrue((ev / "cleaned").exists())
                self.assertIn("command=124 evidence=0", (ev / "exit").read_text())
            finally:
                if proc.poll() is None:
                    proc.kill()
                try:
                    os.kill(int((ev / "child.pid").read_text()), signal.SIGKILL)
                except (OSError, ValueError):
                    pass

    def test_sigterm_to_launcher_tears_down_and_reports(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ev = Path(tmp) / "ev"
            env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
            proc = subprocess.Popen(
                ["bash", str(self.script), "--evidence", str(ev), "--", "bash", "-c",
                 'echo $$ > "$CDB_SESSION_DIR/cmd.pid"; sleep 300'],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env,
            )  # fmt: skip
            try:
                deadline = time.monotonic() + 60
                while not (ev / "cmd.pid").exists() and time.monotonic() < deadline:
                    time.sleep(0.2)
                self.assertTrue((ev / "cmd.pid").exists())
                cmd = int((ev / "cmd.pid").read_text())
                proc.send_signal(signal.SIGTERM)
                self.assertNotEqual(proc.wait(60), 0)
                self.assertTrue((ev / "exit").exists())
                time.sleep(0.5)
                with self.assertRaises(ProcessLookupError):
                    os.kill(cmd, 0)
            finally:
                if proc.poll() is None:
                    proc.kill()

    @unittest.skipUnless(_x11_ready() and _has_gi(), "headless X11 session with GTK required")
    def test_doctor_private_daemon_is_reaped(self) -> None:
        import desktop_runtime as desktop

        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "cua-driver"
            binary.write_text(FAKE_DRIVER)
            binary.chmod(0o755)
            out = Path(tmp) / "out"
            with mock.patch("sys.stdout"):
                code = desktop.main(["doctor", "--cua-bin", str(binary), "--out", str(out)])
            report = json.loads((out / "doctor.json").read_text())
            self.assertEqual(code, 0, report)
            self.assertTrue(report["ok"])
            self.assertEqual(report["daemon"], "private")
            self.assertTrue(report["at_spi_address"])
            with self.assertRaises(ProcessLookupError):
                os.kill(report["daemon_pid"], 0)


SESSION_PY = HERE.parent / "launch" / "linux_session.py"
SESSION_SH = HERE.parent / "launch" / "linux_session.sh"


@unittest.skipUnless(_has_xvfb_dbus(), "Xvfb and dbus-daemon required")
class LinuxSessionSupervisorTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self._stub("openbox", "exec sleep 300")
        self._stub("picom", "exec sleep 300")
        self._stub("xprop", "echo '_NET_SUPPORTING_WM_CHECK(WINDOW): window id # 0x200001'")
        self._stub_gdbus("")
        self.ev = self.tmp / "ev"
        self.procs: list[subprocess.Popen] = []
        self.addCleanup(self._hard_cleanup)

    def _stub(self, name: str, body: str) -> None:
        f = self.bin / name
        f.write_text("#!/bin/sh\n" + body + "\n")
        f.chmod(0o755)

    def _stub_gdbus(self, prefix: str) -> None:
        self._stub("gdbus", prefix + "\necho \"('unix:path=/nonexistent/at-spi',)\"")

    def _env(self, **extra: str) -> dict[str, str]:
        env = {k: v for k, v in os.environ.items() if k != "CDB_BENCH_PID_LOG"}
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        env.update(extra)
        return env

    def _spawn(self, *command: str, args: tuple[str, ...] = (), **env: str) -> subprocess.Popen:
        proc = subprocess.Popen(
            ["bash", str(SESSION_SH), "--evidence", str(self.ev), *args, "--", *command],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=self._env(**env),
        )  # fmt: skip
        self.procs.append(proc)
        return proc

    def _run(self, *command: str, args: tuple[str, ...] = (), **env: str):
        proc = self._spawn(*command, args=args, **env)
        out, _ = proc.communicate(timeout=60)
        return proc.returncode, out

    @staticmethod
    def _session_pids(ev: Path) -> list[int]:
        needle = b"CDB_SESSION_DIR=" + str(ev.resolve()).encode() + b"\0"
        found = []
        for d in Path("/proc").iterdir():
            if not d.name.isdigit() or int(d.name) == os.getpid():
                continue
            try:
                if needle in (d / "environ").read_bytes():
                    found.append(int(d.name))
            except OSError:
                pass
        return found

    def _hard_cleanup(self) -> None:
        for pid in self._session_pids(self.ev):
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
        for proc in self.procs:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
        try:
            self.ev.chmod(0o700)
            (self.ev / "pids.jsonl").chmod(0o600)
        except OSError:
            pass

    def _ledger(self) -> list[dict]:
        return [json.loads(x) for x in (self.ev / "pids.jsonl").read_text().splitlines()]

    def _assert_reaped(self) -> None:
        def gone(call, *a) -> bool:
            try:
                call(*a)
            except ProcessLookupError:
                return True
            return False

        deadline = time.monotonic() + 5
        while True:
            alive = [
                e for e in self._ledger()
                if not (gone(os.kill, e["pid"], 0) and gone(os.killpg, e["pgid"], 0))
            ]  # fmt: skip
            pids = self._session_pids(self.ev)
            if (not alive and not pids) or time.monotonic() > deadline:
                break
            time.sleep(0.1)
        self.assertEqual(alive, [])
        self.assertEqual(pids, [])

    def test_publication_failure_when_command_pid_is_a_directory(self) -> None:
        self.ev.mkdir(mode=0o700)
        (self.ev / "command.pid").mkdir()
        code, out = self._run("sleep", "300")
        self.assertEqual(code, 1, out)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=1 evidence=0")
        self.assertIn("command publication failed", (self.ev / "launch.log").read_text())
        self.assertTrue((self.ev / "command.pid").is_dir())
        ledger = self._ledger()
        self.assertEqual(
            [e["role"] for e in ledger], ["xvfb", "dbus", "openbox", "picom", "command"]
        )
        self.assertTrue(all(e["pgid"] == e["pid"] for e in ledger))
        self.assertEqual(out.splitlines()[-1], f"evidence: {self.ev.resolve()}")
        self._assert_reaped()

    @unittest.skipIf(os.geteuid() == 0, "root ignores file modes")
    def test_unwritable_ledger_after_launch_leaves_no_children(self) -> None:
        self._stub_gdbus('chmod 0444 "$CDB_BENCH_PID_LOG"')
        code, out = self._run("sleep", "300")
        self.assertEqual(code, 1, out)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=1 evidence=0")
        self.assertIn("command publication failed", (self.ev / "launch.log").read_text())
        self.assertEqual([e["role"] for e in self._ledger()], ["xvfb", "dbus", "openbox", "picom"])
        self.assertFalse((self.ev / "command.pid").exists())
        self._assert_reaped()

    def test_sigterm_before_command_starts_never_starts_it(self) -> None:
        self._stub("gdbus", "kill -TERM $PPID; exec sleep 60")
        proc = self._spawn(
            "bash", "-c", 'touch "$CDB_SESSION_DIR/ran"; exec sleep 300'
        )  # fmt: skip
        self.assertEqual(proc.wait(30), 143)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=143 evidence=0")
        self.assertFalse((self.ev / "ran").exists())
        self.assertNotIn("command", [e["role"] for e in self._ledger()])
        self.assertFalse((self.ev / "command.pid").exists())
        self._assert_reaped()

    def test_timeout_kills_a_command_that_ignores_term(self) -> None:
        script = (
            "import os,signal,subprocess,time\n"
            "d=os.environ['CDB_SESSION_DIR']\n"
            "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
            "subprocess.Popen(['sh','-c','trap \"\" TERM; sleep 300'])\n"
            "open(d+'/ready','w').write('1')\n"
            "time.sleep(300)\n"
        )
        proc = self._spawn(
            sys.executable, "-c", script, CDB_SESSION_TIMEOUT="8", CDB_SESSION_GRACE="1"
        )
        self.assertEqual(proc.wait(60), 124)
        self.assertTrue((self.ev / "ready").exists())
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=124 evidence=0")
        self.assertIn("SIGKILL after grace", (self.ev / "launch.log").read_text())
        self._assert_reaped()

    def test_normal_path_environment_auth_and_files(self) -> None:
        self._stub_gdbus("")
        code, out = self._run(
            "bash", "-c",
            'env > "$CDB_SESSION_DIR/env.txt"; '
            'xdotool getdisplaygeometry > "$CDB_SESSION_DIR/geom.txt"; '
            'XAUTHORITY=/dev/null xdotool getdisplaygeometry; '
            'echo $? > "$CDB_SESSION_DIR/noauth.txt"; exit 7',
            AT_SPI_BUS_ADDRESS="unix:path=/nonexistent", WAYLAND_DISPLAY="wayland-9",
        )  # fmt: skip
        self.assertEqual(code, 7, out)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=7 evidence=0")
        self.assertEqual(out.splitlines()[-1], f"evidence: {self.ev.resolve()}")
        self.assertIn("at-spi", (self.ev / "at-spi-address.txt").read_text())
        env = (self.ev / "env.txt").read_text().splitlines()
        self.assertTrue(any(re.fullmatch(r"DISPLAY=:\d+", x) for x in env), env)
        self.assertTrue(any(x.startswith("DBUS_SESSION_BUS_ADDRESS=unix:") for x in env))
        self.assertIn("XDG_SESSION_TYPE=x11", env)
        self.assertTrue(any(x.startswith("XAUTHORITY=") and len(x) > 11 for x in env))
        self.assertFalse(
            any(x.startswith(("AT_SPI_BUS_ADDRESS=", "WAYLAND_DISPLAY=")) for x in env)
        )
        self.assertEqual(
            {e["role"] for e in self._ledger()}, {"xvfb", "dbus", "openbox", "picom", "command"}
        )
        self.assertTrue(all(e["pgid"] == e["pid"] for e in self._ledger()))
        if shutil.which("xdotool"):
            self.assertEqual((self.ev / "geom.txt").read_text().split(), ["1920", "1080"])
            self.assertNotEqual((self.ev / "noauth.txt").read_text().strip(), "0")
        self.assertEqual(stat.S_IMODE(self.ev.stat().st_mode), 0o700)
        self._assert_reaped()

    def test_evidence_status_reuse_and_usage(self) -> None:
        empty = self.tmp / "empty"
        empty.mkdir()
        code, out = self._run("true", args=("--check-run", str(empty)))
        self.assertEqual(code, 1, out)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=0 evidence=1")
        code, out = self._run("bash", "-c", "exit 7", args=("--check-run", str(empty)))
        self.assertEqual(code, 7, out)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=7 evidence=1")
        first = (self.ev / "launch.log").read_text()
        lines = len((self.ev / "pids.jsonl").read_text().splitlines())
        (self.ev / "command.pid").write_text(str(os.getpid()))
        code, out = self._run("true")
        self.assertEqual(code, 0, out)
        self.assertTrue((self.ev / "launch.log").read_text().startswith(first))
        self.assertGreater(len((self.ev / "pids.jsonl").read_text().splitlines()), lines)
        for kw in (
            {"CDB_SESSION_GRACE": "0"},
            {"CDB_SESSION_TIMEOUT": "x"},
            {"CDB_SESSION_TIMEOUT": "nan"},
            {"CDB_SESSION_TIMEOUT": "inf"},
            {"CDB_SESSION_GRACE": "inf"},
            {"CDB_SESSION_GRACE": "nan"},
        ):
            code, _ = self._run("true", **kw)
            self.assertEqual(code, 2)
        done = subprocess.run(
            ["bash", str(SESSION_SH), "--evidence", str(self.ev), "true"],
            capture_output=True, env=self._env(), timeout=30,
        )  # fmt: skip
        self.assertEqual(done.returncode, 2)
        self.assertEqual(self._session_pids(self.ev), [])

    def test_publication_failure_cleanup_survives_unwritable_log(self) -> None:
        self.ev.mkdir(mode=0o700)
        (self.ev / "command.pid").mkdir()
        (self.ev / "launch.log").symlink_to("/dev/full")
        code, out = self._run("sleep", "300")
        self.assertEqual(code, 1, out)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=1 evidence=0")
        self._assert_reaped()

    def test_probe_closing_stdout_does_not_outlive_cancel(self) -> None:
        self._stub("gdbus", "exec 1>&-; kill -TERM $PPID; exec sleep 300")
        proc = self._spawn("true", CDB_SESSION_GRACE="1")
        self.assertEqual(proc.wait(30), 143)
        self.assertEqual((self.ev / "exit").read_text().strip(), "command=143 evidence=0")
        self._assert_reaped()

    def test_probe_closing_stdout_does_not_outlive_timeout(self) -> None:
        self._stub("gdbus", "exec 1>&-; exec sleep 300")
        proc = self._spawn("true", CDB_SESSION_TIMEOUT="8", CDB_SESSION_GRACE="1")
        self.assertEqual(proc.wait(30), 124)
        self._assert_reaped()

    def test_timeout_when_leader_exits_before_term_ignoring_child(self) -> None:
        script = (
            "import os,signal,subprocess,time\n"
            "d=os.environ['CDB_SESSION_DIR']\n"
            "subprocess.Popen(['sh','-c','trap \"\" TERM; sleep 300'])\n"
            "def bye(*a): os._exit(0)\n"
            "signal.signal(signal.SIGTERM,bye)\n"
            "open(d+'/ready','w').write('1')\n"
            "time.sleep(300)\n"
        )
        proc = self._spawn(
            sys.executable, "-c", script, CDB_SESSION_TIMEOUT="8", CDB_SESSION_GRACE="1"
        )
        self.assertEqual(proc.wait(60), 124)
        self.assertTrue((self.ev / "ready").exists())
        self.assertIn("SIGKILL after grace", (self.ev / "launch.log").read_text())
        self._assert_reaped()

    def test_group_exists_when_start_returns(self) -> None:
        import importlib.util

        spec = importlib.util.spec_from_file_location("linux_session", SESSION_PY)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        for _ in range(50):
            p = mod._start_group(["sleep", "5"], log=os.devnull, env=os.environ.copy())
            try:
                self.assertEqual(os.getpgid(p.pid), p.pid)
                self.assertEqual(os.getsid(p.pid), p.pid)
            finally:
                os.killpg(p.pid, signal.SIGKILL)
                p.wait()


if __name__ == "__main__":
    unittest.main()
