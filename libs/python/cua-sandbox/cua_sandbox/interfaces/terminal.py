"""Terminal (PTY) interface, backed by a Transport."""

from __future__ import annotations

from typing import Optional

from cua_sandbox.transport.base import Transport


class Terminal:
    """PTY terminal sessions."""

    def __init__(self, transport: Transport):
        self._t = transport

    async def create(self, shell: str = "bash", cols: int = 80, rows: int = 24) -> dict:
        """Create a new PTY session. Returns {"pid": int, "cols": int, "rows": int}."""
        return await self._t.send("terminal_create", shell=shell, cols=cols, rows=rows)

    async def send_input(self, pid: int, data: str) -> None:
        """Send input to a PTY session."""
        await self._t.send("terminal_send", pid=pid, data=data)

    async def resize(self, pid: int, cols: int, rows: int) -> None:
        """Resize a PTY session."""
        await self._t.send("terminal_resize", pid=pid, cols=cols, rows=rows)

    async def close(self, pid: int) -> Optional[int]:
        """Close a PTY session. Returns exit code."""
        return await self._t.send("terminal_close", pid=pid)
