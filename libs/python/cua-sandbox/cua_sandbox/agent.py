"""Agent integration — adapters that make Sandbox and Localhost work as
AsyncComputerHandler for the cua-agent ComputerAgent.

Usage::

    from cua_sandbox import Image, Sandbox
    from cua_sandbox.agent import SandboxHandler, LocalhostHandler

    async with Sandbox.ephemeral(Image.linux(), local=True) as sb:
        handler = SandboxHandler(sb)
        agent = ComputerAgent(model="...", tools=[handler])
"""

from __future__ import annotations

import asyncio
import base64
from typing import Any, Dict, List, Literal, Optional, Union

from cua_sandbox.localhost import Localhost
from cua_sandbox.sandbox import Sandbox


class SandboxHandler:
    """Adapts a Sandbox instance to the AsyncComputerHandler protocol."""

    def __init__(self, sandbox: Sandbox):
        self._sb = sandbox

    async def get_environment(self) -> Literal["windows", "mac", "linux", "browser"]:
        env = await self._sb.get_environment()
        return env  # type: ignore[return-value]

    async def get_dimensions(self) -> tuple[int, int]:
        return await self._sb.get_dimensions()

    async def screenshot(self, text: Optional[str] = None) -> str:
        raw = await self._sb.screenshot()
        return base64.b64encode(raw).decode("utf-8")

    async def click(self, x: int, y: int, button: str = "left") -> None:
        await self._sb.mouse.click(x, y, button)

    async def double_click(self, x: int, y: int) -> None:
        await self._sb.mouse.double_click(x, y)

    async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
        await self._sb.mouse.scroll(x, y, scroll_x, scroll_y)

    async def type(self, text: str) -> None:
        await self._sb.keyboard.type(text)

    async def wait(self, ms: int = 1000) -> None:
        await asyncio.sleep(ms / 1000.0)

    async def move(self, x: int, y: int) -> None:
        await self._sb.mouse.move(x, y)

    async def keypress(self, keys: Union[List[str], str]) -> None:
        await self._sb.keyboard.keypress(keys)

    async def drag(self, path: List[Dict[str, int]]) -> None:
        if not path:
            return
        start = path[0]
        await self._sb.mouse.mouse_down(start["x"], start["y"])
        for point in path[1:]:
            await self._sb.mouse.move(point["x"], point["y"])
        end = path[-1]
        await self._sb.mouse.mouse_up(end["x"], end["y"])

    async def get_current_url(self) -> str:
        return ""

    async def left_mouse_down(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        if x is not None and y is not None:
            await self._sb.mouse.mouse_down(x, y)

    async def left_mouse_up(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        if x is not None and y is not None:
            await self._sb.mouse.mouse_up(x, y)


class LocalhostHandler(SandboxHandler):
    """Adapts a Localhost instance to the AsyncComputerHandler protocol.

    Localhost exposes the same interface as Sandbox, so we just reuse
    the SandboxHandler with a Localhost in place of a Sandbox.
    """

    def __init__(self, host: Localhost):
        # Localhost has the same interface shape as Sandbox
        self._sb = host  # type: ignore[assignment]


def is_sandbox(obj: Any) -> bool:
    """Check if an object is a Sandbox or Localhost instance."""
    return isinstance(obj, (Sandbox, Localhost))
