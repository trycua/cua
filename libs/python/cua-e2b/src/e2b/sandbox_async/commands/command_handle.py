"""AsyncCommandHandle, with e2b's semantics over a cua-spacesd process."""

from __future__ import annotations

import asyncio
import codecs
import inspect
from typing import List, Optional, Union

from e2b._errors import map_error
from e2b.exceptions import SandboxException, TimeoutException
from e2b.sandbox.commands.command_handle import (
    CommandExitException,
    CommandResult,
    PtyOutput,
    Stderr,
    Stdout,
)
from e2b.sandbox_async.utils import OutputHandler

# What e2b reports (observed against e2b 2.42 sandboxes): a process killed by
# a signal exits with -1 and a Go-style error ("signal: killed"); a non-zero
# exit carries "exit status N".
SIGNAL_EXIT_CODE = -1
# Go's syscall.Signal strings for Linux (zerrors_linux_*.go), keyed by the
# conventional signal name without the "SIG" prefix. Go prints the number for
# an unknown signal ("signal 64"). The shim keys by name, not number, because
# numbers differ between guest operating systems (SIGBUS is 7 on Linux, 10 on
# macOS) while e2b sandboxes are Linux and print Linux's text.
_SIGNAL_TEXT = {
    "hup": "hangup",
    "int": "interrupt",
    "quit": "quit",
    "ill": "illegal instruction",
    "trap": "trace/breakpoint trap",
    "abrt": "aborted",
    "iot": "aborted",
    "bus": "bus error",
    "fpe": "floating point exception",
    "kill": "killed",
    "usr1": "user defined signal 1",
    "segv": "segmentation fault",
    "usr2": "user defined signal 2",
    "pipe": "broken pipe",
    "alrm": "alarm clock",
    "term": "terminated",
    "stkflt": "stack fault",
    "chld": "child exited",
    "cld": "child exited",
    "cont": "continued",
    "stop": "stopped (signal)",
    "tstp": "stopped",
    "ttin": "stopped (tty input)",
    "ttou": "stopped (tty output)",
    "urg": "urgent I/O condition",
    "xcpu": "CPU time limit exceeded",
    "xfsz": "file size limit exceeded",
    "vtalrm": "virtual timer expired",
    "prof": "profiling timer expired",
    "winch": "window changed",
    "io": "I/O possible",
    "poll": "I/O possible",
    "pwr": "power failure",
    "sys": "bad system call",
    # macOS-only names, mapped to the closest Linux text.
    "emt": "illegal instruction",
    "info": "power failure",
}
# Linux signal numbers, for a backend that only knows the number.
_LINUX_SIGNAL_NAME = {
    1: "hup", 2: "int", 3: "quit", 4: "ill", 5: "trap", 6: "abrt", 7: "bus", 8: "fpe",
    9: "kill", 10: "usr1", 11: "segv", 12: "usr2", 13: "pipe", 14: "alrm", 15: "term",
    16: "stkflt", 17: "chld", 18: "cont", 19: "stop", 20: "tstp", 21: "ttin", 22: "ttou",
    23: "urg", 24: "xcpu", 25: "xfsz", 26: "vtalrm", 27: "prof", 28: "winch", 29: "io",
    30: "pwr", 31: "sys",
}  # fmt: skip
DEADLINE_MESSAGE = (
    "Request timed out: This error is likely due to exceeding 'timeout' — the total time a "
    "long running request (like process or directory watch) can be active — or "
    "'request_timeout'. You can modify these by passing 'timeout' or 'request_timeout' when "
    "making the request. Use '0' to disable the timeout."
)


def _signal_text(signal: Optional[str] = None, number: Optional[int] = None) -> str:
    """Go's text for a signal death: "signal: segmentation fault". `signal` is
    the name ("segv", "SIGSEGV", "Kill"), `number` the raw number. The name
    wins because it does not depend on the guest's OS; an unknown signal prints
    like Go does ("signal: signal 64")."""
    name = (signal or "").lower().removeprefix("sig")
    if name not in _SIGNAL_TEXT and number is not None:
        name = _LINUX_SIGNAL_NAME.get(number, "")
        if not name:
            return f"signal: signal {number}"
    return "signal: " + _SIGNAL_TEXT.get(name, name)


class AsyncCommandHandle:
    """Command execution handle: wait for the command, read its output, kill
    it."""

    def __init__(
        self,
        pid: int,
        process,
        on_stdout: Optional[OutputHandler[Stdout]] = None,
        on_stderr: Optional[OutputHandler[Stderr]] = None,
        on_pty: Optional[OutputHandler[PtyOutput]] = None,
        timeout: Optional[float] = None,
    ):
        self._deadline = (asyncio.get_running_loop().time() + timeout) if timeout else None
        self._pid = pid
        self._process = process
        self._stdout_chunks: List[str] = []
        self._stderr_chunks: List[str] = []
        self._stdout_decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        self._stderr_decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        self._on_stdout = on_stdout
        self._on_stderr = on_stderr
        self._on_pty = on_pty
        self._result: Optional[CommandResult] = None
        self._iteration_exception: Optional[BaseException] = None
        self._wait = asyncio.create_task(self._handle_events())

    @property
    def pid(self):
        """Command process ID."""
        return self._pid

    @property
    def stdout(self):
        return "".join(self._stdout_chunks)

    @property
    def stderr(self):
        return "".join(self._stderr_chunks)

    @property
    def error(self):
        return None if self._result is None else self._result.error

    @property
    def exit_code(self):
        """`None` while the command is still running."""
        return None if self._result is None else self._result.exit_code

    async def _emit(self, cb, value):
        if cb is None:
            return
        r = cb(value)
        if inspect.isawaitable(r):
            await r

    async def _flush(self):
        out = self._stdout_decoder.decode(b"", final=True)
        if out:
            self._stdout_chunks.append(out)
        err = self._stderr_decoder.decode(b"", final=True)
        if err:
            self._stderr_chunks.append(err)
        return out, err

    async def _handle_events(self):
        try:
            while True:
                if self._deadline is None:
                    ev = await self._process.next_event()
                else:
                    left = self._deadline - asyncio.get_running_loop().time()
                    try:
                        ev = await asyncio.wait_for(self._process.next_event(), max(left, 0.001))
                    except asyncio.TimeoutError:
                        # the connection's `timeout` ran out; the command keeps running
                        self._iteration_exception = TimeoutException(DEADLINE_MESSAGE)
                        break
                if ev is None:
                    break
                if ev.kind == "stdout" and ev.data:
                    out = self._stdout_decoder.decode(ev.data)
                    if out:
                        self._stdout_chunks.append(out)
                        await self._emit(self._on_stdout, out)
                elif ev.kind == "stderr" and ev.data:
                    err = self._stderr_decoder.decode(ev.data)
                    if err:
                        self._stderr_chunks.append(err)
                        await self._emit(self._on_stderr, err)
                elif ev.kind == "pty" and ev.data:
                    await self._emit(self._on_pty, ev.data)
                elif ev.kind == "exit":
                    out, err = await self._flush()
                    if ev.timed_out:
                        self._iteration_exception = TimeoutException(DEADLINE_MESSAGE)
                    error = ev.error
                    code = ev.exit_code
                    if code is None:
                        code = SIGNAL_EXIT_CODE
                        if ev.signal or ev.signal_name or ev.signal_number:
                            error = error or _signal_text(
                                ev.signal_name or ev.signal, ev.signal_number
                            )
                    elif code != 0:
                        error = error or f"exit status {code}"
                    self._result = CommandResult(
                        stdout="".join(self._stdout_chunks),
                        stderr="".join(self._stderr_chunks),
                        exit_code=code,
                        error=error,
                    )
                    if out:
                        await self._emit(self._on_stdout, out)
                    if err:
                        await self._emit(self._on_stderr, err)
                    break
            if self._result is None:
                await self._flush()
        except asyncio.CancelledError:
            raise
        except Exception as e:  # noqa: BLE001 - surfaced by wait()
            self._iteration_exception = map_error(e)

    async def disconnect(self) -> None:
        """Stops receiving events; the command keeps running."""
        self._wait.cancel()
        await asyncio.wait([self._wait])
        try:
            await asyncio.wait_for(self._process.detach(), 5)
        except Exception:  # noqa: BLE001
            pass

    async def wait(self) -> CommandResult:
        """Waits for the command to finish. Raises `CommandExitException` on a
        non-zero exit code."""
        await self._wait
        if self._iteration_exception:
            raise self._iteration_exception
        if self._result is None:
            raise Exception("Command ended without an end event")
        if self._result.exit_code != 0:
            raise CommandExitException(
                stdout="".join(self._stdout_chunks),
                stderr="".join(self._stderr_chunks),
                exit_code=self._result.exit_code,
                error=self._result.error,
            )
        return self._result

    async def kill(self) -> bool:
        """Kills the command (SIGKILL). `False` if it was not found."""
        try:
            return await self._process.kill()
        except Exception as e:  # noqa: BLE001
            raise map_error(e)

    async def send_stdin(
        self, data: Union[str, bytes], request_timeout: Optional[float] = None
    ) -> None:
        try:
            await self._process.write_stdin(data.encode() if isinstance(data, str) else data)
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=self._pid)

    async def close_stdin(self, request_timeout: Optional[float] = None) -> None:
        try:
            await self._process.close_stdin()
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=self._pid)


__all__ = ["AsyncCommandHandle", "CommandExitException", "CommandResult", "SandboxException"]
