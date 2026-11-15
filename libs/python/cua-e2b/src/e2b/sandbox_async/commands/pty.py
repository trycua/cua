"""`sandbox.pty`: e2b's Pty over cua-spacesd PTY processes."""

from __future__ import annotations

from typing import Dict, Optional

from e2b._errors import map_error
from e2b.sandbox.commands.command_handle import PtyOutput, PtySize
from e2b.sandbox_async.commands.command import (
    _bounded,
    _timeout_ms,
    check_cwd,
    guest_user,
)
from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle
from e2b.sandbox_async.commands.command_input_handle import AsyncCommandInputHandle
from e2b.sandbox_async.utils import OutputHandler


class Pty:
    """Pseudo-terminals in the sandbox (`/bin/bash -i -l`, as e2b)."""

    def __init__(self, machine, envs: Optional[Dict[str, str]] = None, home_of=None):
        self._m = machine
        self._envs = dict(envs or {})
        self._home_of = home_of

    async def create(
        self,
        size: PtySize,
        on_data: OutputHandler[PtyOutput],
        user: Optional[str] = None,
        cwd: Optional[str] = None,
        envs: Optional[Dict[str, str]] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> AsyncCommandHandle:
        env = {**self._envs, **(envs or {})}
        env.setdefault("TERM", "xterm-256color")
        env.setdefault("LANG", "C.UTF-8")
        env.setdefault("LC_ALL", "C.UTF-8")
        try:
            await _bounded(
                check_cwd(self._m, self._home_of, cwd, user, "/bin/bash -i -l"), request_timeout
            )
            proc = await _bounded(
                self._m.spawn(
                    ["/bin/bash", "-i", "-l"],
                    env=env,
                    cwd=cwd,
                    user=guest_user(user),
                    timeout_ms=_timeout_ms(timeout),
                    stdin=True,
                    pty=(size.cols, size.rows),
                ),
                request_timeout,
            )
        except Exception as e:  # noqa: BLE001
            raise map_error(e, user=user)
        return AsyncCommandHandle(pid=proc.pid, process=proc, on_pty=on_data)

    async def connect(
        self,
        pid: int,
        on_data: OutputHandler[PtyOutput],
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> AsyncCommandHandle:
        try:
            proc = await _bounded(self._m.attach(pid), request_timeout)
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=pid)
        return AsyncCommandHandle(pid=proc.pid, process=proc, on_pty=on_data, timeout=timeout)

    def stream_input(self, pid: int, *, timeout: Optional[float] = 60.0) -> AsyncCommandInputHandle:
        """Opens an ordered input handle to a PTY (see `Commands.stream_input`)."""
        return AsyncCommandInputHandle(lambda: self._m.attach(pid), "pty", timeout, pid)

    async def _with(self, pid, request_timeout, fn):
        try:
            proc = await _bounded(self._m.attach(pid), request_timeout)
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=pid)
        try:
            return await fn(proc)
        except Exception as e:  # noqa: BLE001
            raise map_error(e, process=pid)
        finally:
            await proc.detach()

    async def send_stdin(
        self, pid: int, data: bytes, request_timeout: Optional[float] = None
    ) -> None:
        await self._with(pid, request_timeout, lambda p: p.write_pty(bytes(data)))

    async def resize(
        self, pid: int, size: PtySize, request_timeout: Optional[float] = None
    ) -> None:
        await self._with(pid, request_timeout, lambda p: p.resize(size.cols, size.rows))

    async def kill(self, pid: int, request_timeout: Optional[float] = None) -> bool:
        try:
            proc = await _bounded(self._m.attach(pid), request_timeout)
        except Exception as e:  # noqa: BLE001
            if type(e).__name__ == "NotFound":
                return False
            raise map_error(e, process=pid)
        try:
            return await proc.kill()
        finally:
            await proc.detach()
