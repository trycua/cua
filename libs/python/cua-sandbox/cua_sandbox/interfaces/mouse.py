"""Mouse interface — click, move, drag, scroll, backed by a Transport."""

from __future__ import annotations


from cua_sandbox.transport.base import Transport


class Mouse:
    """Mouse control."""

    def __init__(self, transport: Transport):
        self._t = transport

    async def click(self, x: int, y: int, button: str = "left") -> None:
        await self._t.send("left_click", x=x, y=y, button=button)

    async def right_click(self, x: int, y: int) -> None:
        await self._t.send("right_click", x=x, y=y)

    async def double_click(self, x: int, y: int) -> None:
        await self._t.send("double_click", x=x, y=y)

    async def move(self, x: int, y: int) -> None:
        await self._t.send("move_cursor", x=x, y=y)

    async def scroll(self, x: int, y: int, scroll_x: int = 0, scroll_y: int = 3) -> None:
        await self._t.send("scroll", x=x, y=y, scroll_x=scroll_x, scroll_y=scroll_y)

    async def mouse_down(self, x: int, y: int, button: str = "left") -> None:
        await self._t.send("mouse_down", x=x, y=y, button=button)

    async def mouse_up(self, x: int, y: int, button: str = "left") -> None:
        await self._t.send("mouse_up", x=x, y=y, button=button)

    async def drag(
        self, start_x: int, start_y: int, end_x: int, end_y: int, button: str = "left"
    ) -> None:
        await self._t.send(
            "drag", start_x=start_x, start_y=start_y, end_x=end_x, end_y=end_y, button=button
        )
