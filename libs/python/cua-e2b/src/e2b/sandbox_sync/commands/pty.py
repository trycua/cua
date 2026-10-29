"""Pty: the synchronous `sandbox.pty`."""

from __future__ import annotations

from typing import Dict, Iterable, Optional, Union

from e2b.exceptions import InvalidArgumentException
from e2b.sandbox.commands.command_handle import PtySize
from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle
from e2b.sandbox_async.commands.pty import Pty as AsyncPty
from e2b.sandbox_sync._loop import Loop, proxy
from e2b.sandbox_sync.commands.command_handle import CommandHandle

STREAM_INPUT_TIMEOUT = 60.0


class Pty:
    """Pseudo-terminals in the sandbox. Read a terminal's output with
    `handle.wait(on_pty=...)` or by iterating the handle."""

    def __init__(self, inner: AsyncPty):
        self._p = inner

    kill = proxy("_p", AsyncPty.kill)
    resize = proxy("_p", AsyncPty.resize)
    send_stdin = proxy("_p", AsyncPty.send_stdin)

    def create(
        self,
        size: PtySize,
        user: Optional[str] = None,
        cwd: Optional[str] = None,
        envs: Optional[Dict[str, str]] = None,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> CommandHandle:
        # The async create starts the shell; its handle must exist on the loop
        # with the queue callbacks, so build it there.
        import queue

        from e2b.sandbox_sync.commands.command_handle import _END

        q: "queue.Queue" = queue.Queue()

        async def start() -> AsyncCommandHandle:
            h = await self._p.create(
                size,
                lambda data: q.put((None, None, data)),
                user=user,
                cwd=cwd,
                envs=envs,
                timeout=timeout,
                request_timeout=request_timeout,
            )
            h._wait.add_done_callback(lambda _t: q.put(_END))
            return h

        return CommandHandle(Loop.run(start()), q)

    def connect(
        self,
        pid: int,
        timeout: Optional[float] = 60,
        request_timeout: Optional[float] = None,
    ) -> CommandHandle:
        import queue

        from e2b.sandbox_sync.commands.command_handle import _END

        q: "queue.Queue" = queue.Queue()

        async def start() -> AsyncCommandHandle:
            h = await self._p.connect(
                pid,
                lambda data: q.put((None, None, data)),
                timeout=timeout,
                request_timeout=request_timeout,
            )
            h._wait.add_done_callback(lambda _t: q.put(_END))
            return h

        return CommandHandle(Loop.run(start()), q)

    def stream_input(
        self,
        pid: int,
        data: Iterable[Union[str, bytes]],
        *,
        timeout: Optional[float] = STREAM_INPUT_TIMEOUT,
    ) -> None:
        """Sends `data` (an iterable of chunks) to the terminal in order."""
        if isinstance(data, (str, bytes, bytearray, memoryview)):
            raise InvalidArgumentException(
                "`data` must be an iterable of str or bytes chunks, not a single "
                "chunk; wrap it in a list, e.g. `stream_input(pid, [data])`."
            )

        async def send():
            handle = self._p.stream_input(pid, timeout=timeout)
            for chunk in data:
                await handle.send(chunk)
            await handle.close()

        Loop.run(send())


__all__ = ["Pty"]
