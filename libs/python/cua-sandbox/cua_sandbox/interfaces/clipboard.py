"""Clipboard interface, backed by a Transport."""

from __future__ import annotations

from cua_sandbox.transport.base import Transport


class Clipboard:
    """Clipboard read/write."""

    def __init__(self, transport: Transport):
        self._t = transport

    async def get(self) -> str:
        """Return the current clipboard text."""
        result = await self._t.send("copy_to_clipboard")
        if isinstance(result, dict):
            return result.get("content", "")
        return result

    async def set(self, text: str) -> None:
        """Set the clipboard text."""
        await self._t.send("set_clipboard", text=text)
