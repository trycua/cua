"""
Computer handler implementation for OpenAI computer-use-preview protocol.
"""

import base64
from typing import Any, Dict, List, Literal, Optional, Union

from computer import Computer

from .base import AsyncComputerHandler


class cuaComputerHandler(AsyncComputerHandler):
    """Computer handler that implements the Computer protocol using the computer interface."""

    def __init__(self, cua_computer: Computer):
        """Initialize with a computer interface (from tool schema)."""
        self.cua_computer = cua_computer
        self.interface = None

    async def _initialize(self):
        if hasattr(self.cua_computer, "_initialized") and not self.cua_computer._initialized:
            await self.cua_computer.run()
        self.interface = self.cua_computer.interface

    # ==== Computer-Use-Preview Action Space ====

    async def get_environment(self) -> Literal["windows", "mac", "linux", "browser"]:
        """Get the current environment type."""
        # TODO: detect actual environment
        return "linux"

    async def get_dimensions(self) -> tuple[int, int]:
        """Get screen dimensions as (width, height)."""
        assert self.interface is not None
        screen_size = await self.interface.get_screen_size()
        return screen_size["width"], screen_size["height"]

    async def screenshot(self, text: Optional[str] = None) -> str:
        """Take a screenshot and return as base64 string.

        Args:
            text: Optional descriptive text (for compatibility with GPT-4o models, ignored)
        """
        assert self.interface is not None
        screenshot_bytes = await self.interface.screenshot()
        return base64.b64encode(screenshot_bytes).decode("utf-8")

    async def click(self, x: int, y: int, button: str = "left") -> None:
        """Click at coordinates with specified button."""
        assert self.interface is not None
        if button == "left":
            await self.interface.left_click(x, y)
        elif button == "right":
            await self.interface.right_click(x, y)
        else:
            # Default to left click for unknown buttons
            await self.interface.left_click(x, y)

    async def double_click(self, x: int, y: int) -> None:
        """Double click at coordinates."""
        assert self.interface is not None
        await self.interface.double_click(x, y)

    async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
        """Scroll at coordinates with specified scroll amounts."""
        assert self.interface is not None
        await self.interface.move_cursor(x, y)
        await self.interface.scroll(scroll_x, scroll_y)

    async def type(self, text: str) -> None:
        """Type text."""
        assert self.interface is not None
        await self.interface.type_text(text)

    async def wait(self, ms: int = 1000) -> None:
        """Wait for specified milliseconds."""
        assert self.interface is not None
        import asyncio

        await asyncio.sleep(ms / 1000.0)

    async def move(self, x: int, y: int) -> None:
        """Move cursor to coordinates."""
        assert self.interface is not None
        await self.interface.move_cursor(x, y)

    async def keypress(self, keys: Union[List[str], str]) -> None:
        """Press key combination."""
        assert self.interface is not None
        if isinstance(keys, str):
            keys = keys.replace("-", "+").split("+")
        if len(keys) == 1:
            await self.interface.press_key(keys[0])
        else:
            # Handle key combinations
            await self.interface.hotkey(*keys)

    async def drag(self, path: List[Dict[str, int]]) -> None:
        """Drag along specified path."""
        assert self.interface is not None
        if not path:
            return

        # Start drag from first point
        start = path[0]
        await self.interface.mouse_down(start["x"], start["y"])

        # Move through path
        for point in path[1:]:
            await self.interface.move_cursor(point["x"], point["y"])

        # End drag at last point
        end = path[-1]
        await self.interface.mouse_up(end["x"], end["y"])

    async def get_current_url(self) -> str:
        """Get current URL (for browser environments)."""
        # This would need to be implemented based on the specific browser interface
        # For now, return empty string
        return ""

    # ==== Anthropic Computer Action Space ====
    async def left_mouse_down(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse down at coordinates."""
        assert self.interface is not None
        await self.interface.mouse_down(x, y, button="left")

    async def left_mouse_up(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse up at coordinates."""
        assert self.interface is not None
        await self.interface.mouse_up(x, y, button="left")
