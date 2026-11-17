"""Window interface, backed by a Transport."""

from __future__ import annotations

from cua_sandbox.transport.base import Transport


class Window:
    """Window management."""

    def __init__(self, transport: Transport):
        self._t = transport

    async def get_active_title(self) -> str:
        """Return the title of the currently focused window ("" when none).

        cua-spacesd answers from WindowsService.ListWindows (the focused
        window); computer-server never implemented this command.
        """
        result = await self._t.send("get_active_window_title")
        if isinstance(result, dict):
            result = result.get("title", result.get("result", ""))
        return result or ""
