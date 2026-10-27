#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Usage: linux_session.py [--evidence DIR] [--check-run DIR]... -- COMMAND [ARG...]
#
# Single-owner supervisor for a headless X11 session. Every component and the command
# start through _start_group (start_new_session=True): setsid() runs in the child before
# exec and Popen returns after exec, so the supervisor only ever holds a pid whose process
# group already exists with the same id. Every cleanup path targets a known group.
from __future__ import annotations

import contextlib
import os
import secrets
import select
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent.parent
READY_LIMIT = 15.0


class State:
    cancel = 0
    deadline: float | None = None


state = State()


def stopping() -> bool:
    return bool(state.cancel) or (state.deadline is not None and time.monotonic() >= state.deadline)


def _timed_out() -> bool:
    return state.deadline is not None and time.monotonic() >= state.deadline


def _on_term(*_a) -> None:
    state.cancel = state.cancel or 143


def _on_int(*_a) -> None:
    state.cancel = state.cancel or 130


def _usage() -> None:
    print(
        "usage: linux_session.sh [--evidence DIR] [--check-run DIR]... -- COMMAND [ARG...]",
        file=sys.stderr,
    )
    sys.exit(2)


def _start_group(argv, *, log, env, stdin=subprocess.DEVNULL, pass_fds=()):
    with open(log, "ab") as out:
        return subprocess.Popen(
            list(argv), stdin=stdin, stdout=out, stderr=subprocess.STDOUT, env=env,
            start_new_session=True, close_fds=True, pass_fds=pass_fds,
        )  # fmt: skip


def _ledger(path, pid: int, role: str) -> None:
    import json

    with open(path, "a") as f:
        f.write(json.dumps({"pid": pid, "pgid": pid, "role": role}) + "\n")


def _group_gone(pid: int) -> bool:
    try:
        os.killpg(pid, 0)
    except ProcessLookupError:
        return True
    return False


def _stop(proc, grace: float, role: str = "process", log=None) -> None:
    with contextlib.suppress(ProcessLookupError):
        os.killpg(proc.pid, signal.SIGTERM)
    end = time.monotonic() + grace
    while time.monotonic() < end:
        if proc.poll() is not None and _group_gone(proc.pid):
            break
        time.sleep(0.05)
    if not (proc.poll() is not None and _group_gone(proc.pid)):
        with contextlib.suppress(ProcessLookupError):
            os.killpg(proc.pid, signal.SIGKILL)
        if log:
            _log(log, f"{role}: SIGKILL after grace")
    proc.wait()


def _log(path, msg: str) -> None:
    # Diagnostics are best-effort: a full or unwritable log must never skip cleanup.
    with contextlib.suppress(OSError), open(path, "a") as f:
        f.write(msg + "\n")


def _read_line(fd: int, what: str) -> str:
    buf = b""
    end = time.monotonic() + READY_LIMIT
    try:
        while b"\n" not in buf:
            if stopping() or time.monotonic() > end:
                raise RuntimeError(f"{what}: not ready")
            r, _, _ = select.select([fd], [], [], 0.2)
            if r:
                chunk = os.read(fd, 4096)
                if not chunk:
                    raise RuntimeError(f"{what}: exited before reporting readiness")
                buf += chunk
    finally:
        os.close(fd)
    return buf.split(b"\n", 1)[0].decode().strip()


def _probe(argv, timeout: float, env, log) -> bytes | None:
    r, w = os.pipe()
    try:
        proc = subprocess.Popen(
            argv, stdin=subprocess.DEVNULL, stdout=w, stderr=subprocess.DEVNULL, env=env,
            start_new_session=True, close_fds=True,
        )  # fmt: skip
    except OSError:
        os.close(r)
        os.close(w)
        return None
    os.close(w)
    out = b""
    end = time.monotonic() + timeout
    try:
        while True:
            if stopping() or time.monotonic() > end:
                _stop(proc, 0)
                return None
            ready, _, _ = select.select([r], [], [], 0.1)
            if ready:
                chunk = os.read(r, 4096)
                if not chunk:
                    break
                out += chunk
        # EOF on stdout does not mean the probe exited: keep polling so cancel and timeout win.
        while proc.poll() is None:
            if stopping() or time.monotonic() > end:
                _stop(proc, 0)
                return None
            time.sleep(0.05)
        _stop(proc, 0)
        return out if proc.returncode == 0 else None
    finally:
        os.close(r)
        if proc.poll() is None:
            _stop(proc, 0)


def _write_cookie(path: Path) -> None:
    def s(b: bytes) -> bytes:
        return struct.pack(">H", len(b)) + b

    data = struct.pack(">H", 0xFFFF) + s(b"") + s(b"") + s(b"MIT-MAGIC-COOKIE-1")
    data += s(secrets.token_bytes(16))
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as f:
        f.write(data)


def _setup_evidence(arg: str | None) -> tuple[Path, bool]:
    new = False
    if not arg:
        root = Path(os.environ.get("CDB_BENCH_WORK") or Path.home() / ".cache/cua-bench-h2h")
        root = root / "linux-sessions"
        root.mkdir(parents=True, exist_ok=True)
        ev = Path(tempfile.mkdtemp(prefix="run-", dir=root))
        new = True
    else:
        ev = Path(arg)
        if not ev.exists():
            ev.mkdir(mode=0o700, parents=True)
            new = True
    ev = ev.resolve()
    git = subprocess.run(
        ["git", "-C", str(ev), "rev-parse", "--show-toplevel"], capture_output=True
    )
    if git.returncode == 0:
        print(f"evidence directory must be outside a git checkout: {ev}", file=sys.stderr)
        if new:
            with contextlib.suppress(OSError):
                ev.rmdir()
        sys.exit(2)
    ev.chmod(0o700)
    return ev, new


def _parse_args(argv):
    evidence = None
    checks: list[str] = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("--evidence", "--check-run"):
            if i + 1 >= len(argv):
                _usage()
            if a == "--evidence":
                evidence = argv[i + 1]
            else:
                checks.append(argv[i + 1])
            i += 2
        elif a == "--":
            i += 1
            break
        else:
            _usage()
    command = argv[i:]
    if not command:
        _usage()
    return evidence, checks, command


def _env_float(name: str, default: float, *, positive: bool) -> float:
    raw = os.environ.get(name)
    if raw in (None, ""):
        return default
    try:
        v = float(raw)
    except ValueError:
        v = -1.0
    if v < 0 or (positive and v <= 0):
        print(f"invalid {name}: {raw}", file=sys.stderr)
        sys.exit(2)
    return v


def main(argv) -> int:
    if sys.platform != "linux":
        print("linux_session.sh requires Linux", file=sys.stderr)
        return 2
    evidence_arg, checks, command = _parse_args(argv)
    timeout = _env_float("CDB_SESSION_TIMEOUT", 0.0, positive=False)
    grace = _env_float("CDB_SESSION_GRACE", 10.0, positive=True)
    ev, new = _setup_evidence(evidence_arg)
    ledger = ev / "pids.jsonl"
    os.environ["CDB_SESSION_DIR"] = str(ev)
    os.environ["CDB_BENCH_PID_LOG"] = str(ledger)
    if new:
        ledger.write_text("")
    for stale in ("command.pid",):
        with contextlib.suppress(FileNotFoundError, IsADirectoryError):
            (ev / stale).unlink()
    log = ev / "launch.log"
    offset = log.stat().st_size if log.exists() else 0

    signal.signal(signal.SIGTERM, _on_term)
    signal.signal(signal.SIGINT, _on_int)
    if timeout > 0:
        state.deadline = time.monotonic() + timeout

    components: list[tuple[str, subprocess.Popen]] = []
    runtime = Path(tempfile.mkdtemp(prefix="cdb-session-"))
    try:
        status = _run_session(ev, ledger, log, runtime, command, grace, components)
    finally:
        for role, proc in reversed(components):
            _stop(proc, grace, role, log)
        shutil.rmtree(runtime, ignore_errors=True)
    return _finish(ev, ledger, log, offset, checks, status)


def _start_failed_status() -> int:
    if _timed_out():
        return 124
    return state.cancel or 1


def _run_session(ev, ledger, log, runtime, command, grace, components) -> int:
    try:
        env = _start_components(ev, ledger, log, runtime, components)
    except (OSError, RuntimeError) as e:
        _log(log, f"session start failed: {e}")
        return _start_failed_status() if stopping() else 1
    if env is None:
        return _start_failed_status() if stopping() else 1
    if stopping():
        return _start_failed_status()
    try:
        cmd = _start_group(command, log=log, env=env, stdin=None)
    except FileNotFoundError as e:
        _log(log, f"cannot start command: {e}")
        return 127
    except OSError as e:
        _log(log, f"cannot start command: {e}")
        return 126
    # Publication failure: nothing outside the supervisor knows this group, kill it now.
    try:
        _ledger(ledger, cmd.pid, "command")
        (ev / "command.pid").write_text(f"{cmd.pid}\n")
    except OSError as e:
        try:
            os.killpg(cmd.pid, signal.SIGKILL)
            cmd.wait()
        finally:
            _log(log, f"command publication failed: {e}")
        return 1
    while True:
        try:
            cmd.wait(timeout=0.1)
            break
        except subprocess.TimeoutExpired:
            if stopping():
                break
    interrupted = cmd.poll() is None
    if interrupted:
        _stop(cmd, grace, "command", log)
    rc = cmd.returncode
    code = rc if rc >= 0 else 128 - rc
    if _timed_out() and interrupted:
        code = 124
    elif state.cancel and (code == 0 or rc < 0):
        code = state.cancel
    # Leftovers are reported by the check first, then killed.
    _LEAK.append(cmd.pid)
    return code


_LEAK: list[int] = []


def _start_components(ev, ledger, log, runtime, components):
    def add(role, proc):
        components.append((role, proc))
        _ledger(ledger, proc.pid, role)
        return proc

    auth = runtime / "Xauthority"
    _write_cookie(auth)
    base = os.environ.copy()
    for k in ("WAYLAND_DISPLAY", "AT_SPI_BUS_ADDRESS"):
        base.pop(k, None)

    r, w = os.pipe()
    try:
        xvfb = _start_group(
            ["Xvfb", "-displayfd", str(w), "-auth", str(auth), "-nolisten", "tcp",
             "-screen", "0", "1920x1080x24"],
            log=ev / "xvfb.log", env=base, pass_fds=(w,),
        )  # fmt: skip
    except OSError:
        os.close(r)
        os.close(w)
        raise
    os.close(w)
    components.append(("xvfb", xvfb))
    _ledger(ledger, xvfb.pid, "xvfb")
    num = _read_line(r, "Xvfb")
    if not num.isdigit():
        raise RuntimeError(f"Xvfb reported bad display: {num!r}")
    env = base
    env.update(DISPLAY=f":{num}", XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11")

    r, w = os.pipe()
    try:
        dbus = _start_group(
            ["dbus-daemon", "--session", "--nofork", "--nopidfile",
             f"--address=unix:tmpdir={runtime}", f"--print-address={w}"],
            log=ev / "dbus.log", env=env, pass_fds=(w,),
        )  # fmt: skip
    except OSError:
        os.close(r)
        os.close(w)
        raise
    os.close(w)
    components.append(("dbus", dbus))
    _ledger(ledger, dbus.pid, "dbus")
    addr = _read_line(r, "dbus-daemon")
    if not addr.startswith("unix:"):
        raise RuntimeError(f"dbus-daemon reported bad address: {addr!r}")
    env["DBUS_SESSION_BUS_ADDRESS"] = addr

    add("openbox", _start_group(["openbox"], log=ev / "openbox.log", env=env))
    add(
        "picom",
        _start_group(["picom", "--backend", "xrender", "--config", "/dev/null"],
                     log=ev / "picom.log", env=env),
    )  # fmt: skip

    end = time.monotonic() + READY_LIMIT
    while True:
        out = _probe(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"], 5, env, log)
        if out and b"window id" in out:
            break
        if stopping():
            return None
        if time.monotonic() > end:
            raise RuntimeError("window manager not ready")
        time.sleep(0.2)
    for role, proc in components:
        if proc.poll() is not None:
            raise RuntimeError(f"session component {role} exited early")
    addr_out = _probe(
        ["gdbus", "call", "--session", "--dest", "org.a11y.Bus", "--object-path",
         "/org/a11y/bus", "--method", "org.a11y.Bus.GetAddress"],
        15, env, log,
    )  # fmt: skip
    if addr_out is None:
        if stopping():
            return None
        raise RuntimeError("accessibility bus not available")
    (ev / "at-spi-address.txt").write_bytes(addr_out)
    return env


def _finish(ev, ledger, log, offset, checks, status) -> int:
    with contextlib.suppress(OSError), open(log, "rb") as f:
        if not stat.S_ISREG(os.fstat(f.fileno()).st_mode):
            raise OSError("launch.log is not a regular file")
        f.seek(offset)
        sys.stdout.write(f.read().decode(errors="replace"))
        sys.stdout.flush()
    check = BENCH / "tools" / "check_run_evidence.py"
    evidence = 0
    if subprocess.run([sys.executable, str(check), "--ledger-only", str(ledger)]).returncode:
        evidence = 1
    for run in checks:
        if subprocess.run([sys.executable, str(check), run]).returncode:
            evidence = 1
    for pid in _LEAK:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(pid, signal.SIGKILL)
    (ev / "exit").write_text(f"command={status} evidence={evidence}\n")
    print(f"evidence: {ev}")
    sys.stdout.flush()
    return 1 if status == 0 and evidence else status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
