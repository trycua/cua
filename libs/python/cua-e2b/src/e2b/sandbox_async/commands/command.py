"""`sandbox.commands`: e2b's Commands over cua-spacesd's process service."""

from __future__ import annotations

import asyncio
import posixpath
from typing import Dict, List, Literal, Optional, Union, overload

from e2b._errors import map_error
from e2b.exceptions import InvalidArgumentException, TimeoutException
from e2b.sandbox.commands.command_handle import CommandResult
from e2b.sandbox.commands.main import ProcessInfo
from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle, Stderr, Stdout
from e2b.sandbox_async.commands.command_input_handle import AsyncCommandInputHandle
from e2b.sandbox_async.utils import OutputHandler

Username = str
DEFAULT_REQUEST_TIMEOUT = 60.0
_U32_MAX = 2**32 - 1


def _timeout_ms(timeout: Optional[float]) -> Optional[int]:
    """e2b: `0` (or None) does not limit the command."""
    if not timeout:
        return None
    return min(int(timeout * 1000), _U32_MAX)


def guest_user(user: Optional[Username]) -> Optional[str]:
    """e2b's `"user"` is the template's default user: here, the Space's."""
    return None if user in (None, "", "user") else user


async def _bounded(coro, request_timeout: Optional[float]):
    t = DEFAULT_REQUEST_TIMEOUT if request_timeout is None else request_timeout
    if not t:
        return await coro
    try:
        return await asyncio.wait_for(coro, t)
    except asyncio.TimeoutError:
        raise TimeoutException(
            "Request timed out — the 'request_timeout' option can be used to increase this timeout"
        ) from None


async def check_cwd(machine, home_of, cwd: Optional[str], user: Optional[str], program: str):
    """e2b's start errors for a working directory that is missing or not a
    directory, raised before the command starts (as e2b's start event fails)
    so a background command fails in `run()` too. A relative `cwd` is
    relative to the user's home. Only a definite answer is acted on: a
    directory the driver cannot stat is left to the guest."""
    if not cwd:
        return
    path = cwd
    if not posixpath.isabs(path) and home_of is not None:
        try:
            path = posixpath.join(await home_of(user), path)
        except Exception:  # noqa: BLE001
            return
    try:
        st = await machine.stat(path)
    except Exception as e:  # noqa: BLE001
        if type(e).__name__ in ("NotFound", "FileNotFoundError"):
            raise InvalidArgumentException(f"cwd '{path}' does not exist") from None
        return
    if st.type == "file" or st.type == "other":
        raise InvalidArgumentException(
            f"error starting process '{program}': fork/exec /bin/sh: not a directory"
        )


class Commands:
    """Executes commands in the sandbox."""

    def __init__(self, machine, envs: Optional[Dict[str, str]] = None, home_of=None):
        self._m = machine
        self._envs = dict(envs or {})
        self._home_of = home_of

    async def list(self, request_timeout: Optional[float] = None) -> List[ProcessInfo]:
        try:
            procs = await _bounded(self._m.list_processes(), request_timeout)
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        out = []
        for p in procs:
            spec = p.get("config") or {}
            out.append(
                ProcessInfo(
                    pid=int(p.get("pid", 0)),
                    tag=p.get("tag") or None,
                    cmd=spec.get("command", ""),
                    args=list(spec.get("args", [])),
                    envs=dict(spec.get("env", {})),
                    cwd=spec.get("cwd") or None,
                )
            )
        return out

    async def kill(self, pid: int, request_timeout: Optional[float] = None) -> bool:
        """SIGKILLs a command. `False` when it was not found."""
        try:
            proc = await _bounded(self._m.attach(pid), request_timeout)
        except Exception as e:  # noqa: BLE001
            mapped = map_error(e, process=pid)
            if type(e).__name__ == "NotFound":
                return False
            raise mapped
        try:
            return await proc.kill()
        finally:
            await proc.detach()

    async def send_stdin(
        self, pid: int, data: Union[str, bytes], request_timeout: Optional[float] = None
    ) -> None:
        try:
            proc = await _bounded(self._m.attach(pid), request_timeout)
            try:
                await proc.write_stdin(data.encode() if isinstance(data, str) else data)
            finally:
                await proc.detach()
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=pid)

    async def close_stdin(self, pid: int, request_timeout: Optional[float] = None) -> None:
        try:
            proc = await _bounded(self._m.attach(pid), request_timeout)
            try:
                await proc.close_stdin()
            finally:
                await proc.detach()
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=pid)

    def stream_input(self, pid: int, *, timeout: Optional[float] = 60.0) -> AsyncCommandInputHandle:
        """Opens an ordered input handle to the command's stdin: `send()`
        queues data (delivered in order), `close()` ends the stream (it does
        not close the command's stdin: `close_stdin` does)."""
        return AsyncCommandInputHandle(lambda: self._m.attach(pid), "stdin", timeout, pid)

    @overload
    async def run(
        self,
        cmd: str,
        background: Union[Literal[False], None] = None,
        envs: Optional[Dict[str, str]] = None,
        user: Optional[Username] = None,
        cwd: Optional[str] = None,
        on_stdout: Optional[OutputHandler[Stdout]] = None,
        on_stderr: Optional[OutputHandler[Stderr]] = None,
        stdin: Optional[bool] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> CommandResult: ...

    @overload
    async def run(
        self,
        cmd: str,
        background: Literal[True],
        envs: Optional[Dict[str, str]] = None,
        user: Optional[Username] = None,
        cwd: Optional[str] = None,
        on_stdout: Optional[OutputHandler[Stdout]] = None,
        on_stderr: Optional[OutputHandler[Stderr]] = None,
        stdin: Optional[bool] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> AsyncCommandHandle: ...

    async def run(
        self,
        cmd: str,
        background: Union[bool, None] = None,
        envs: Optional[Dict[str, str]] = None,
        user: Optional[Username] = None,
        cwd: Optional[str] = None,
        on_stdout: Optional[OutputHandler[Stdout]] = None,
        on_stderr: Optional[OutputHandler[Stderr]] = None,
        stdin: Optional[bool] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ):
        """Runs `cmd` with `/bin/bash -l -c`, as e2b does.

        Foreground: waits and returns a `CommandResult`; a non-zero exit
        raises `CommandExitException`, and running past `timeout` seconds
        kills the command and raises `TimeoutException`. Background: returns
        an `AsyncCommandHandle`. `timeout=0` does not limit the command.
        """
        proc = await self._start(cmd, envs, user, cwd, bool(stdin), timeout, request_timeout)
        handle = AsyncCommandHandle(
            pid=proc.pid, process=proc, on_stdout=on_stdout, on_stderr=on_stderr
        )
        return handle if background else await handle.wait()

    async def _start(self, cmd, envs, user, cwd, stdin, timeout, request_timeout):
        try:
            await _bounded(
                check_cwd(self._m, self._home_of, cwd, user, f"/bin/bash -l -c {cmd}"),
                request_timeout,
            )
            return await _bounded(
                self._m.spawn(
                    ["/bin/bash", "-l", "-c", cmd],
                    env={**self._envs, **(envs or {})},
                    cwd=cwd,
                    user=guest_user(user),
                    timeout_ms=_timeout_ms(timeout),
                    stdin=stdin,
                ),
                request_timeout,
            )
        except Exception as e:  # noqa: BLE001
            raise map_error(e, user=user)

    async def connect(
        self,
        pid: int,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
        on_stdout: Optional[OutputHandler[Stdout]] = None,
        on_stderr: Optional[OutputHandler[Stderr]] = None,
    ) -> AsyncCommandHandle:
        """Attaches to a running command (new output only)."""
        try:
            proc = await _bounded(self._m.attach(pid), request_timeout)
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=pid)
        return AsyncCommandHandle(
            pid=proc.pid, process=proc, on_stdout=on_stdout, on_stderr=on_stderr, timeout=timeout
        )
