"""
Computer handler implementation wrapping a cua_sandbox.Sandbox instance.
"""

import asyncio
from typing import Any, Dict, List, Literal, Optional, Union

from .base import AsyncComputerHandler


class SandboxComputerHandler(AsyncComputerHandler):
    """Computer handler that adapts a cua_sandbox.Sandbox to the AsyncComputerHandler protocol."""

    def __init__(self, sandbox: Any):
        self._sandbox = sandbox

    # ==== Computer-Use-Preview Action Space ====

    async def get_environment(self) -> Literal["windows", "mac", "linux", "browser"]:
        return await self._sandbox.get_environment()

    async def get_dimensions(self) -> tuple[int, int]:
        return await self._sandbox.get_dimensions()

    async def screenshot(self, text: Optional[str] = None) -> str:
        return await self._sandbox.screenshot_base64()

    async def click(self, x: int, y: int, button: str = "left") -> None:
        if button == "right":
            await self._sandbox.mouse.right_click(x, y)
        else:
            await self._sandbox.mouse.click(x, y, button=button)

    async def double_click(self, x: int, y: int) -> None:
        await self._sandbox.mouse.double_click(x, y)

    async def right_click(self, x: int, y: int) -> None:
        await self._sandbox.mouse.right_click(x, y)

    async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
        await self._sandbox.mouse.scroll(x, y, scroll_x=scroll_x, scroll_y=scroll_y)

    async def type(self, text: str) -> None:
        await self._sandbox.keyboard.type(text)

    async def wait(self, ms: int = 1000) -> None:
        await asyncio.sleep(ms / 1000.0)

    async def move(self, x: int, y: int) -> None:
        await self._sandbox.mouse.move(x, y)

    # Maps Anthropic/X11 key names → pynput Key attribute names used by computer-server
    _KEY_NAME_MAP = {
        "return": "enter",
        "backspace": "backspace",
        "delete": "delete",
        "del": "delete",
        "escape": "esc",
        "esc": "esc",
        "tab": "tab",
        "space": "space",
        " ": "space",
        "ctrl": "ctrl",
        "control": "ctrl",
        "shift": "shift",
        "alt": "alt",
        "super": "cmd",
        "meta": "cmd",
        "cmd": "cmd",
        "command": "cmd",
        "win": "cmd",
        "up": "up",
        "down": "down",
        "left": "left",
        "right": "right",
        "home": "home",
        "end": "end",
        "pageup": "page_up",
        "page_up": "page_up",
        "pgup": "page_up",
        "pagedown": "page_down",
        "page_down": "page_down",
        "pgdn": "page_down",
        "insert": "insert",
        "ins": "insert",
        "caps_lock": "caps_lock",
        **{f"f{i}": f"f{i}" for i in range(1, 21)},
    }

    async def keypress(self, keys: Union[List[str], str]) -> None:
        if isinstance(keys, str):
            keys = [keys]
        normalized = []
        for k in keys:
            mapped = self._KEY_NAME_MAP.get(k.lower(), k.lower() if len(k) > 1 else k)
            normalized.append(mapped)
        await self._sandbox.keyboard.keypress(normalized)

    async def drag(
        self,
        path: Optional[List[Dict[str, int]]] = None,
        start_x: Optional[int] = None,
        start_y: Optional[int] = None,
        end_x: Optional[int] = None,
        end_y: Optional[int] = None,
    ) -> None:
        if start_x is not None and start_y is not None and end_x is not None and end_y is not None:
            path = [{"x": start_x, "y": start_y}, {"x": end_x, "y": end_y}]
        if not path:
            return
        await self._sandbox.mouse.drag(path)

    async def get_current_url(self) -> str:
        return ""

    async def terminate(self, status: str = "success") -> Dict[str, Any]:
        return {"success": True, "status": status, "terminated": True}

    # ==== Anthropic Action Space ====

    async def left_mouse_down(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        await self._sandbox.mouse.mouse_down(x, y, button="left")

    async def left_mouse_up(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        await self._sandbox.mouse.mouse_up(x, y, button="left")
