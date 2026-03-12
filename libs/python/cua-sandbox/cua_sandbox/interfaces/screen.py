"""Screen interface — screenshots and screen info, backed by a Transport."""

from __future__ import annotations

import base64
from typing import Dict, Tuple

from cua_sandbox.transport.base import Transport


class Screen:
    """Screen capture and info."""

    def __init__(self, transport: Transport):
        self._t = transport

    async def screenshot(self) -> bytes:
        """Capture a screenshot and return raw PNG bytes."""
        return await self._t.screenshot()

    async def screenshot_base64(self) -> str:
        """Capture a screenshot and return as a base64-encoded string."""
        raw = await self._t.screenshot()
        return base64.b64encode(raw).decode("ascii")

    async def size(self) -> Tuple[int, int]:
        """Return (width, height) of the screen."""
        d = await self._t.get_screen_size()
        return d["width"], d["height"]
