"""Terminal (PTY) interface, backed by a Transport."""

from __future__ import annotations

from typing import Optional

from cua_sandbox.transport.base import Transport


class Terminal:
    """PTY terminal sessions."""

    def __init__(self, transport: Transport):
        self._t = transport

    async def create(self, command: Optional[str] = None, cols: int = 80, rows: int = 24) -> dict:
        """Create a new PTY session. Returns {"pid": int, "cols": int, "rows": int}."""
        return await self._t.pty_create(command=command, cols=cols, rows=rows)

    async def send_input(self, pid: int, data: str) -> None:
        """Send input to a PTY session."""
        await self._t.pty_send(pid, data)

    async def info(self, pid: int) -> Optional[dict]:
        """Return session info or None if gone."""
        return await self._t.pty_info(pid)

    async def close(self, pid: int) -> bool:
        """Kill a PTY session."""
        return await self._t.pty_kill(pid)
