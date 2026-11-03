"""Commands: the synchronous `sandbox.commands`."""

from __future__ import annotations

from typing import Callable, Dict, Iterable, List, Literal, Optional, Union, overload

from e2b._errors import map_error
from e2b.exceptions import InvalidArgumentException
from e2b.sandbox.commands.command_handle import CommandResult
from e2b.sandbox.commands.main import ProcessInfo
from e2b.sandbox_async.commands.command import Commands as AsyncCommands
from e2b.sandbox_async.commands.command import _bounded, _timeout_ms, guest_user
from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle
from e2b.sandbox_sync._loop import Loop, proxy
from e2b.sandbox_sync.commands.command_handle import CommandHandle

Username = str
STREAM_INPUT_TIMEOUT = 60.0


class Commands:
    """Executes commands in the sandbox."""

    def __init__(self, inner: AsyncCommands):
        self._c = inner

    list = proxy("_c", AsyncCommands.list)
    kill = proxy("_c", AsyncCommands.kill)
    send_stdin = proxy("_c", AsyncCommands.send_stdin)
    close_stdin = proxy("_c", AsyncCommands.close_stdin)

    @overload
    def run(
        self,
        cmd: str,
        background: Union[Literal[False], None] = None,
        envs: Optional[Dict[str, str]] = None,
        user: Optional[Username] = None,
        cwd: Optional[str] = None,
        on_stdout: Optional[Callable[[str], None]] = None,
        on_stderr: Optional[Callable[[str], None]] = None,
        stdin: Optional[bool] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> CommandResult: ...

    @overload
    def run(
        self,
        cmd: str,
        background: Literal[True],
        envs: Optional[Dict[str, str]] = None,
        user: Optional[Username] = None,
        cwd: Optional[str] = None,
        on_stdout: None = None,
        on_stderr: None = None,
        stdin: Optional[bool] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> CommandHandle: ...

    def run(
        self,
        cmd: str,
        background: Union[bool, None] = None,
        envs: Optional[Dict[str, str]] = None,
        user: Optional[Username] = None,
        cwd: Optional[str] = None,
        on_stdout: Optional[Callable[[str], None]] = None,
        on_stderr: Optional[Callable[[str], None]] = None,
        stdin: Optional[bool] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ):
        """Runs `cmd` with `/bin/bash -l -c`. Foreground: waits and returns a
        `CommandResult` (a non-zero exit raises `CommandExitException`; the
        callbacks run on the cua-e2b loop thread). Background: returns a
        `CommandHandle`."""
        if not background:
            return Loop.run(
                self._c.run(
                    cmd,
                    None,
                    envs,
                    user,
                    cwd,
                    on_stdout,
                    on_stderr,
                    stdin,
                    timeout,
                    request_timeout,
                )
            )

        async def start() -> AsyncCommandHandle:
            proc = await self._c._start(cmd, envs, user, cwd, bool(stdin), timeout, request_timeout)
            return proc

        proc = Loop.run(start())
        return CommandHandle._attach(
            lambda **cb: _make(proc, cb), on_stdout=on_stdout, on_stderr=on_stderr
        )

    def connect(
        self,
        pid: int,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> CommandHandle:
        """Attaches to a running command (new output only)."""

        async def attach():
            try:
                return await _bounded(self._c._m.attach(pid), request_timeout)
            except Exception as e:  # noqa: BLE001
                raise map_error(e, process=pid)

        proc = Loop.run(attach())
        return CommandHandle._attach(lambda **cb: _make(proc, cb))

    def stream_input(
        self,
        pid: int,
        data: Iterable[Union[str, bytes]],
        *,
        timeout: Optional[float] = STREAM_INPUT_TIMEOUT,
    ) -> None:
        """Sends `data` (an iterable of chunks) to the command's stdin in
        order; returns once all of it was delivered."""
        if isinstance(data, (str, bytes, bytearray, memoryview)):
            raise InvalidArgumentException(
                "`data` must be an iterable of str or bytes chunks, not a single "
                "chunk; wrap it in a list, e.g. `stream_input(pid, [data])`."
            )

        async def send():
            handle = self._c.stream_input(pid, timeout=timeout)
            for chunk in data:
                await handle.send(chunk)
            await handle.close()

        Loop.run(send())


def _make(proc, cb) -> AsyncCommandHandle:
    return AsyncCommandHandle(
        pid=proc.pid,
        process=proc,
        on_stdout=cb["on_stdout"],
        on_stderr=cb["on_stderr"],
        on_pty=cb["on_pty"],
    )


__all__ = ["Commands"]
