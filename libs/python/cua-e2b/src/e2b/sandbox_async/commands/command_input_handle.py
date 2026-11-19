"""AsyncCommandInputHandle: an ordered, long-lived input stream to a command
or a PTY (e2b 2.58 `commands.stream_input(pid)` / `pty.stream_input(pid)`)."""

from __future__ import annotations

import asyncio
from typing import Any, Awaitable, Callable, Literal, Optional, Union

from e2b._errors import map_error
from e2b.exceptions import SandboxException, TimeoutException

InputKind = Literal["stdin", "pty"]

_CLOSED = (
    "Cannot send input after the input handle is closed. Open a new one "
    "with `sandbox.commands.stream_input(pid)` or "
    "`sandbox.pty.stream_input(pid)`."
)
DEADLINE = (
    "This error is likely due to exceeding 'timeout' — the total time a long running request "
    "(like process or directory watch) can be active — or 'request_timeout'. You can modify "
    "these by passing 'timeout' or 'request_timeout' when making the request. Use '0' to "
    "disable the timeout."
)


class _Close:
    pass


_CLOSE = _Close()


class AsyncCommandInputHandle:
    """Queue input with `send()` (delivered strictly in order) and end the
    stream with `close()`, which returns once everything queued has been
    delivered. Usable as an async context manager (closes on exit).

    Closing the handle ends the input stream only: it does not send EOF to the
    command (use `commands.close_stdin(pid)` for that), as in e2b.
    """

    def __init__(
        self,
        attach: Callable[[], Awaitable[Any]],
        input_kind: InputKind,
        timeout: Optional[float],
        pid: Optional[int] = None,
    ) -> None:
        self._pid = pid
        self._attach = attach
        self._kind = input_kind
        self._timeout = timeout or None
        self._queue: asyncio.Queue = asyncio.Queue()
        self._closing = False
        self._stopped = False
        self._failure: Optional[BaseException] = None
        self._task = asyncio.create_task(self._run())

    async def send(self, data: Union[str, bytes]) -> None:
        """Queues `data` for ordered delivery (returns once queued).

        :raises NotFoundException: the command or PTY does not exist (surfaced
            here or by `close()`).
        :raises TimeoutException: the stream timeout expired.
        :raises SandboxException: the handle is closed.
        """
        if self._failure is not None:
            raise self._failure
        if self._closing or self._stopped:
            raise SandboxException(_CLOSED)
        self._queue.put_nowait(data)

    async def close(self) -> None:
        """Ends the input stream and waits until everything queued was
        delivered."""
        if not self._closing:
            self._closing = True
            self._queue.put_nowait(_CLOSE)
        await asyncio.shield(self._task)
        if self._failure is not None:
            raise self._failure

    async def __aenter__(self) -> "AsyncCommandInputHandle":
        return self

    async def __aexit__(self, exc_type: Any, exc_value: Any, traceback: Any) -> None:
        await self.close()

    async def _pump(self, proc) -> None:
        while True:
            item = await self._queue.get()
            if isinstance(item, _Close):
                return
            raw = item.encode("utf-8") if isinstance(item, str) else bytes(item)
            if self._kind == "pty":
                await proc.write_pty(raw)
            else:
                await proc.write_stdin(raw)

    async def _run(self) -> None:
        proc = None
        try:
            proc = await self._attach()
            try:
                await asyncio.wait_for(self._pump(proc), self._timeout)
            except asyncio.TimeoutError:
                raise TimeoutException(f"deadline_exceeded: {DEADLINE}") from None
        except asyncio.CancelledError:
            raise
        except Exception as e:  # noqa: BLE001
            self._failure = map_error(e, process=self._pid)
            self._discard()
        finally:
            self._stopped = True
            if proc is not None:
                try:
                    await proc.detach()
                except Exception:  # noqa: BLE001
                    pass

    def _discard(self) -> None:
        while not self._queue.empty():
            try:
                self._queue.get_nowait()
            except asyncio.QueueEmpty:
                break
