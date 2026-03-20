"""Localhost — wraps cua_auto directly. No sandbox, no computer-server.

All cua_auto calls are sync; async wrappers use asyncio.to_thread().

Usage::

    from cua_sandbox import localhost

    async with localhost() as host:
        await host.mouse.click(100, 200)
        img = await host.screen.screenshot()
"""

from __future__ import annotations

from contextlib import asynccontextmanager
from typing import Any, AsyncIterator, Optional

from cua_sandbox.interfaces import (
    Clipboard,
    Keyboard,
    Mouse,
    Screen,
    Shell,
    Terminal,
    Window,
)
from cua_sandbox.transport.local import LocalTransport


class Localhost:
    """Direct host control via cua_auto — no sandboxing."""

    def __init__(self) -> None:
        self._transport = LocalTransport()
        self.screen = Screen(self._transport)
        self.mouse = Mouse(self._transport)
        self.keyboard = Keyboard(self._transport)
        self.clipboard = Clipboard(self._transport)
        self.shell = Shell(self._transport)
        self.window = Window(self._transport)
        self.terminal = Terminal(self._transport)

    async def _connect(self) -> None:
        await self._transport.connect()

    async def disconnect(self) -> None:
        await self._transport.disconnect()

    async def screenshot(self, text: Optional[str] = None) -> bytes:
        return await self._transport.screenshot()

    async def screenshot_base64(self, text: Optional[str] = None) -> str:
        return await self.screen.screenshot_base64()

    async def get_environment(self) -> str:
        return await self._transport.get_environment()

    async def get_dimensions(self) -> tuple[int, int]:
        return await self.screen.size()

    async def __aenter__(self) -> Localhost:
        await self._connect()
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.disconnect()

    def __repr__(self) -> str:
        return "Localhost()"


@asynccontextmanager
async def localhost() -> AsyncIterator[Localhost]:
    """Async context manager yielding a Localhost instance."""
    host = Localhost()
    await host._connect()
    try:
        yield host
    finally:
        await host.disconnect()
