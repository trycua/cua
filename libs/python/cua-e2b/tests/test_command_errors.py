"""Error classes and texts of command, PTY and stdin calls (e2b's, as recorded
by the conformance goldens), over the in-memory backend."""

from __future__ import annotations

import pytest
from e2b import (
    AsyncSandbox,
    AuthenticationException,
    InvalidArgumentException,
    NotFoundException,
    PtySize,
)
from e2b._backend import CuaProcess, set_backend
from e2b._errors import map_error
from fake_backend import CuaError, FakeBackend


@pytest.fixture
def backend():
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


async def _sandbox(backend):
    sb = await AsyncSandbox.create(template="w")
    return sb, backend.machines[sb.sandbox_id]


async def test_missing_cwd_is_invalid_argument_before_anything_runs(backend):
    sb, m = await _sandbox(backend)
    with pytest.raises(InvalidArgumentException) as e:
        await sb.commands.run("pwd", cwd="/no/such/dir")
    assert str(e.value) == "cwd '/no/such/dir' does not exist"
    assert not any(s["cwd"] == "/no/such/dir" for s in m.spawned)


async def test_missing_cwd_fails_a_background_run_too(backend):
    sb, _ = await _sandbox(backend)
    with pytest.raises(InvalidArgumentException):
        await sb.commands.run("pwd", cwd="/no/such/dir", background=True)


async def test_relative_cwd_is_relative_to_the_home(backend):
    sb, m = await _sandbox(backend)
    with pytest.raises(InvalidArgumentException) as e:
        await sb.commands.run("pwd", cwd="tmp")
    assert str(e.value) == "cwd '/home/cua/tmp' does not exist"
    m.dirs.add("/home/cua/tmp")
    await sb.commands.run("echo ok", cwd="tmp")


async def test_cwd_that_is_a_file_reads_as_e2bs_start_error(backend):
    sb, m = await _sandbox(backend)
    m.files["/etc/passwd"] = b"root"
    with pytest.raises(InvalidArgumentException) as e:
        await sb.commands.run("pwd", cwd="/etc/passwd")
    assert str(e.value) == (
        "error starting process '/bin/bash -l -c pwd': fork/exec /bin/sh: not a directory"
    )


async def test_pty_create_checks_the_cwd_too(backend):
    sb, _ = await _sandbox(backend)
    with pytest.raises(InvalidArgumentException):
        await sb.pty.create(PtySize(80, 24), on_data=lambda d: None, cwd="/no/such/dir")


def test_unknown_user_is_authentication_exception_with_e2bs_text():
    err = CuaError(
        'env: rpc failed: user "nosuchuser": no such user "nosuchuser" (InvalidArgument)'
    )
    mapped = map_error(err, user="nosuchuser")
    assert isinstance(mapped, AuthenticationException)
    assert str(mapped) == "invalid username: 'nosuchuser'"


@pytest.mark.parametrize(
    "msg",
    [
        "env: rpc failed: stdin is closed or was not requested (set StartProcessRequest.stdin) (FailedPrecondition)",
        "env: rpc failed: process has exited (FailedPrecondition)",
    ],
)
def test_refused_stdin_is_not_found_with_the_pid(msg):
    e = map_error(CuaError(msg), process=1234)
    assert isinstance(e, NotFoundException)
    assert str(e) == "process with pid 1234 not found"


async def test_handle_send_and_close_stdin_errors_name_the_pid(backend):
    sb, m = await _sandbox(backend)
    h = await sb.commands.run("forever", background=True, stdin=True)
    proc = m.procs[h.pid]

    async def refuse(*_a):
        raise CuaError("env: rpc failed: process has exited (FailedPrecondition)")

    proc.write_stdin = refuse
    proc.close_stdin = refuse
    for call in (h.send_stdin("x"), h.close_stdin()):
        with pytest.raises(NotFoundException) as e:
            await call
        assert str(e.value) == f"process with pid {h.pid} not found"
    await h.kill()


class _Native:
    """A native process whose stdin closes once, as cua-spacesd's does."""

    def __init__(self):
        self.closed = False
        self.calls = []

    def pid(self):
        return 7

    async def write_stdin(self, data):
        self.calls.append(("write", data))
        if self.closed:
            raise CuaError("env: rpc failed: stdin is closed (FailedPrecondition)")

    async def close_stdin(self):
        self.calls.append(("close",))
        self.closed = True


async def test_closing_stdin_twice_is_refused_like_e2b():
    p = CuaProcess(_Native())
    await p.close_stdin()
    with pytest.raises(CuaError) as e:
        await p.close_stdin()
    mapped = map_error(e.value, process=p.pid)
    assert isinstance(mapped, NotFoundException)
    assert str(mapped) == "process with pid 7 not found"
    assert p._p.calls == [("write", b""), ("close",), ("write", b"")]


async def test_dropped_output_is_an_error_not_a_short_result():
    """cua-spacesd's scrollback ring drops output for a reader that falls
    behind; the offsets show the gap and the shim must not hide it."""
    import types

    from e2b import SandboxException
    from e2b._backend import CuaProcess
    from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle

    def ev(offset, data):
        return types.SimpleNamespace(
            kind=types.SimpleNamespace(name="STDOUT"), offset=offset, data=data, exit=None
        )

    class Raw:
        def __init__(self):
            self.events = [ev(0, b"aaaa"), ev(4, b"bbbb"), ev(100, b"cccc")]

        def pid(self):
            return 5

        async def next_event(self):
            return self.events.pop(0) if self.events else None

    h = AsyncCommandHandle(pid=5, process=CuaProcess(Raw()))
    with pytest.raises(SandboxException, match="92 bytes of command output were dropped"):
        await h.wait()


# ── death by signal: Go's syscall.Signal text for every Linux signal ──

GO_LINUX_SIGNAL_TEXT = {
    1: "hangup", 2: "interrupt", 3: "quit", 4: "illegal instruction",
    5: "trace/breakpoint trap", 6: "aborted", 7: "bus error",
    8: "floating point exception", 9: "killed", 10: "user defined signal 1",
    11: "segmentation fault", 12: "user defined signal 2", 13: "broken pipe",
    14: "alarm clock", 15: "terminated", 16: "stack fault", 17: "child exited",
    18: "continued", 19: "stopped (signal)", 20: "stopped",
    21: "stopped (tty input)", 22: "stopped (tty output)",
    23: "urgent I/O condition", 24: "CPU time limit exceeded",
    25: "file size limit exceeded", 26: "virtual timer expired",
    27: "profiling timer expired", 28: "window changed", 29: "I/O possible",
    30: "power failure", 31: "bad system call",
}  # fmt: skip
NAMES = {
    1: "HUP", 2: "INT", 3: "QUIT", 4: "ILL", 5: "TRAP", 6: "ABRT", 7: "BUS", 8: "FPE",
    9: "KILL", 10: "USR1", 11: "SEGV", 12: "USR2", 13: "PIPE", 14: "ALRM", 15: "TERM",
    16: "STKFLT", 17: "CHLD", 18: "CONT", 19: "STOP", 20: "TSTP", 21: "TTIN", 22: "TTOU",
    23: "URG", 24: "XCPU", 25: "XFSZ", 26: "VTALRM", 27: "PROF", 28: "WINCH", 29: "IO",
    30: "PWR", 31: "SYS",
}  # fmt: skip


@pytest.mark.parametrize("number", sorted(GO_LINUX_SIGNAL_TEXT))
def test_signal_text_is_gos_for_every_linux_signal(number):
    from e2b.sandbox_async.commands.command_handle import _signal_text

    want = "signal: " + GO_LINUX_SIGNAL_TEXT[number]
    assert _signal_text(NAMES[number], number) == want
    assert _signal_text("SIG" + NAMES[number], None) == want
    assert _signal_text(None, number) == want


def test_signal_text_prefers_the_name_over_a_foreign_number():
    from e2b.sandbox_async.commands.command_handle import _signal_text

    # SIGBUS is 10 on macOS, which is SIGUSR1 on Linux: the name decides.
    assert _signal_text("BUS", 10) == "signal: bus error"
    assert _signal_text(None, 64) == "signal: signal 64"


async def test_a_signal_the_contract_enum_lacks_still_reports_its_text(backend):
    from e2b._backend import ProcEvent
    from e2b.sandbox.commands.command_handle import CommandExitException
    from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle

    class One:
        pid = 1

        def __init__(self):
            # What cua-spacesd sends for SIGSEGV: no enum signal, raw fields only.
            self.events = [ProcEvent(kind="exit", signal_name="SEGV", signal_number=11)]

        async def next_event(self):
            return self.events.pop(0) if self.events else None

        async def kill(self):
            return True

        async def detach(self):
            pass

    with pytest.raises(CommandExitException) as info:
        await AsyncCommandHandle(pid=1, process=One()).wait()
    assert (info.value.exit_code, info.value.error) == (-1, "signal: segmentation fault")
