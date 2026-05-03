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
from typing import AsyncIterator, Optional

from cua_sandbox.interfaces import (
    Clipboard,
    Keyboard,
    Mouse,
    Screen,
    Shell,
    Terminal,
    Window,
)
from cua_sandbox.sandbox import _ConnectResult
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

    @classmethod
    def connect(cls) -> "_ConnectResult":
        """Connect to the local machine.

        Supports both ``await`` and ``async with``.

        Examples::

            # plain await
            host = await Localhost.connect()
            await host.shell.run("echo hello")
            await host.disconnect()

            # context manager
            async with Localhost.connect() as host:
                await host.shell.run("echo hello")
        """

        async def _factory() -> "Localhost":
            host = cls()
            await host._connect()
            return host

        return _ConnectResult(_factory)

    def __repr__(self) -> str:
        return "Localhost()"


@asynccontextmanager
async def localhost() -> AsyncIterator[Localhost]:
    """Async context manager yielding a Localhost instance.

    .. deprecated::
        Prefer ``Localhost.connect()`` instead.
    """
    async with Localhost.connect() as host:
        yield host
