"""CommandHandle: the synchronous handle on a running command."""

from __future__ import annotations

import queue
from typing import Callable, Optional, Union

from e2b.sandbox.commands.command_handle import (
    CommandExitException,
    CommandResult,
    PtyOutput,
    Stderr,
    Stdout,
)
from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle
from e2b.sandbox_sync._loop import Loop

_END = object()


class CommandHandle:
    """Command execution handle: wait for the command, read its output as it
    arrives (`for stdout, stderr, pty in handle`), kill it.

    The output is pumped by the asynchronous implementation on its background
    thread and handed over through a queue, so a callback you pass runs on
    your thread (inside `wait()` or the iteration), not on that one.
    """

    def __init__(self, inner: AsyncCommandHandle, events: "queue.Queue"):
        self._h = inner
        self._events = events
        self._iteration_done = False

    @classmethod
    def _attach(cls, make: Callable[..., AsyncCommandHandle], **callbacks) -> "CommandHandle":
        """Builds the async handle with callbacks that feed the queue."""
        q: "queue.Queue" = queue.Queue()
        user_out, user_err, user_pty = (
            callbacks.get("on_stdout"),
            callbacks.get("on_stderr"),
            callbacks.get("on_pty"),
        )

        def out(text):
            q.put((text, None, None))
            if user_out:
                user_out(text)

        def err(text):
            q.put((None, text, None))
            if user_err:
                user_err(text)

        def pty(data):
            q.put((None, None, data))
            if user_pty:
                user_pty(data)

        async def build() -> AsyncCommandHandle:
            h = make(on_stdout=out, on_stderr=err, on_pty=pty)  # needs the running loop
            h._wait.add_done_callback(lambda _t: q.put(_END))
            return h

        return cls(Loop.run(build()), q)

    pid = property(lambda self: self._h.pid)
    # e2b's synchronous handle has no live `stdout`/`stderr`/`error`/`exit_code`
    # (read them from the `CommandResult` that `wait()` returns, or the
    # exception it raises); the asynchronous handle does.

    def __iter__(self):
        """Yields `(stdout, stderr, pty)` tuples (one is set) until the
        command ends; a failure of the stream is raised at the end."""
        while not self._iteration_done:
            item = self._events.get()
            if item is _END:
                self._iteration_done = True
                self._events.put(_END)  # a second iteration ends at once
                break
            yield item
        if self._h._iteration_exception is not None:
            raise self._h._iteration_exception

    def wait(
        self,
        on_pty: Optional[Callable[[PtyOutput], None]] = None,
        on_stdout: Optional[Callable[[Stdout], None]] = None,
        on_stderr: Optional[Callable[[Stderr], None]] = None,
    ) -> CommandResult:
        """Waits for the command and returns its result; a non-zero exit code
        raises `CommandExitException`."""
        for stdout, stderr, pty in self:
            if stdout is not None and on_stdout:
                on_stdout(stdout)
            elif stderr is not None and on_stderr:
                on_stderr(stderr)
            elif pty is not None and on_pty:
                on_pty(pty)
        return Loop.run(self._h.wait())

    def kill(self) -> bool:
        """Kills the command (SIGKILL). `False` if it was not found."""
        return Loop.run(self._h.kill())

    def disconnect(self) -> None:
        """Stops receiving events; the command keeps running."""
        Loop.run(self._h.disconnect())
        self._events.put(_END)

    def send_stdin(self, data: Union[str, bytes], request_timeout: Optional[float] = None) -> None:
        Loop.run(self._h.send_stdin(data, request_timeout))

    def close_stdin(self, request_timeout: Optional[float] = None) -> None:
        Loop.run(self._h.close_stdin(request_timeout))


__all__ = ["CommandExitException", "CommandHandle", "CommandResult"]
