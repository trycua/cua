"""Async PTY session manager for computer-server.

Wraps :class:`cua_auto.terminal.Terminal` and adds asyncio-compatible
output broadcasting via per-consumer :class:`asyncio.Queue` objects.

Queue items are dicts:
  - ``{"type": "output", "data": <bytes>}``  — terminal output chunk
  - ``{"type": "exit", "code": <int>}``      — session terminated (sentinel)
"""

from __future__ import annotations

import asyncio
import logging
import threading
from typing import Dict, List, Optional

logger = logging.getLogger(__name__)


class PtyManager:
    """Manage PTY sessions with async broadcast queues.

    Usage::

        mgr = PtyManager()

        # Create a session
        info = await mgr.create(command="bash", cols=80, rows=24)
        pid = info["pid"]

        # Subscribe to output
        q = mgr.subscribe(pid)
        async for msg in _drain(q):
            ...  # msg is {"type": "output", "data": b"..."} or {"type": "exit", "code": 0}

        # Write to stdin
        await mgr.send_stdin(pid, b"echo hello\\n")

        # Resize
        await mgr.resize(pid, 120, 40)

        # Kill
        await mgr.kill(pid)
    """

    def __init__(self) -> None:
        # pid → list of subscriber queues
        self._queues: Dict[int, List[asyncio.Queue]] = {}
        # pid → {"cols": int, "rows": int}
        self._sessions: Dict[int, dict] = {}
        self._terminal = None

    # ------------------------------------------------------------------
    # Lazy terminal access
    # ------------------------------------------------------------------

    def _get_terminal(self):
        if self._terminal is None:
            from cua_auto.terminal import Terminal

            self._terminal = Terminal()
        return self._terminal

    # ------------------------------------------------------------------
    # Public async API
    # ------------------------------------------------------------------

    async def create(
        self,
        command: Optional[str] = None,
        cols: int = 80,
        rows: int = 24,
        cwd: Optional[str] = None,
        envs: Optional[dict] = None,
    ) -> dict:
        """Spawn a new PTY session.

        Returns:
            ``{"pid": int, "cols": int, "rows": int}``
        """
        loop = asyncio.get_running_loop()
        terminal = self._get_terminal()

        # We need pid to route output, but we only know it after create().
        # Use a mutable cell so the callback can look up the pid after creation.
        # early_buffer holds chunks that arrive before pid_cell[0] is set (the
        # reader thread can fire before asyncio.to_thread returns the session).
        pid_cell: List[Optional[int]] = [None]
        early_buffer: List[bytes] = []

        def _on_data(data: bytes) -> None:
            pid = pid_cell[0]
            if pid is None:
                early_buffer.append(data)
                return
            msg = {"type": "output", "data": data}
            for q in list(self._queues.get(pid, [])):
                try:
                    loop.call_soon_threadsafe(q.put_nowait, msg)
                except Exception:
                    pass

        session = await asyncio.to_thread(
            terminal.create,
            command=command,
            cols=cols,
            rows=rows,
            on_data=_on_data,
            cwd=cwd,
            envs=envs,
        )

        pid_cell[0] = session.pid
        self._queues[session.pid] = []
        self._sessions[session.pid] = {"cols": session.cols, "rows": session.rows}

        # Flush any output that arrived before pid_cell[0] was set.
        for chunk in early_buffer:
            msg = {"type": "output", "data": chunk}
            for q in list(self._queues[session.pid]):
                try:
                    loop.call_soon_threadsafe(q.put_nowait, msg)
                except Exception:
                    pass

        # Watch for process exit in a daemon thread, then broadcast sentinel.
        def _watch_exit() -> None:
            exit_code = terminal.wait(session.pid) or 0
            sentinel = {"type": "exit", "code": exit_code}
            for q in list(self._queues.get(session.pid, [])):
                try:
                    loop.call_soon_threadsafe(q.put_nowait, sentinel)
                except Exception:
                    pass

        threading.Thread(target=_watch_exit, daemon=True, name=f"pty-exit-{session.pid}").start()

        logger.info("PTY session created: pid=%d cmd=%r cols=%d rows=%d", session.pid, command, cols, rows)
        return {"pid": session.pid, "cols": session.cols, "rows": session.rows}

    async def send_stdin(self, pid: int, data: bytes) -> None:
        """Write *data* to the stdin of session *pid*."""
        terminal = self._get_terminal()
        await asyncio.to_thread(terminal.send_stdin, pid, data)

    async def resize(self, pid: int, cols: int, rows: int) -> None:
        """Resize the terminal for session *pid*."""
        terminal = self._get_terminal()
        await asyncio.to_thread(terminal.resize, pid, cols, rows)
        if pid in self._sessions:
            self._sessions[pid]["cols"] = cols
            self._sessions[pid]["rows"] = rows

    def get_info(self, pid: int) -> Optional[dict]:
        """Return ``{"pid": int, "cols": int, "rows": int}`` for *pid*, or ``None`` if unknown."""
        info = self._sessions.get(pid)
        if info is None:
            return None
        return {"pid": pid, "cols": info["cols"], "rows": info["rows"]}

    async def kill(self, pid: int) -> bool:
        """Kill session *pid*.

        Also broadcasts the exit sentinel to all subscribers.
        Returns:
            ``True`` if the signal was delivered.
        """
        terminal = self._get_terminal()
        result = await asyncio.to_thread(terminal.kill, pid)
        # Broadcast sentinel immediately (the exit watcher will also fire,
        # but duplicate sentinels are harmless — consumers stop after the first).
        sentinel = {"type": "exit", "code": -1}
        for q in list(self._queues.get(pid, [])):
            try:
                q.put_nowait(sentinel)
            except Exception:
                pass
        return result

    # ------------------------------------------------------------------
    # Queue-based pub/sub
    # ------------------------------------------------------------------

    def subscribe(self, pid: int) -> asyncio.Queue:
        """Return a new :class:`asyncio.Queue` that will receive output for *pid*.

        The queue receives dicts with keys ``type`` (``"output"`` or ``"exit"``)
        and either ``data`` (bytes) or ``code`` (int).
        """
        q: asyncio.Queue = asyncio.Queue()
        self._queues.setdefault(pid, []).append(q)
        return q

    def unsubscribe(self, pid: int, queue: asyncio.Queue) -> None:
        """Remove *queue* from the subscriber list for *pid*."""
        qs = self._queues.get(pid, [])
        try:
            qs.remove(queue)
        except ValueError:
            pass
