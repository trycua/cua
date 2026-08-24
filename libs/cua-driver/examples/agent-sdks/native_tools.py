"""Narrow agent-tool adapter over the same-process Cua Driver Python SDK."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from typing import TypeVar

from cua_driver import (
    ActionTarget,
    ClickButton,
    ClickInput,
    CuaDriver,
    GetDesktopStateInput,
    PressKeyInput,
    ToolResult,
    TypeTextInput,
)

ToolContent = dict[str, object]
T = TypeVar("T")


class NativeDesktopTools:
    """Own one in-process runtime with one implicit lifecycle session."""

    def __init__(self, timeout: float = 30.0) -> None:
        self.driver = CuaDriver.create()
        self.timeout = timeout

    async def close(self) -> None:
        await self.driver.shutdown()

    async def observe(self) -> dict[str, object]:
        result = await self._bounded(
            self.driver.get_desktop_state(
                GetDesktopStateInput(
                    session=None,
                    screenshot_out_file=None,
                )
            )
        )
        return self._content(result)

    async def click(self, x: float, y: float) -> dict[str, object]:
        return await self._mutate_then_observe(
            lambda: self.driver.click(
                ClickInput(
                    x=x,
                    y=y,
                    target=ActionTarget.DESKTOP(display_id="primary"),
                    scope=None,
                    session=None,
                    button=ClickButton.LEFT,
                    count=1,
                )
            )
        )

    async def type_text(self, text: str) -> dict[str, object]:
        return await self._mutate_then_observe(
            lambda: self.driver.type_text(
                TypeTextInput(
                    text=text,
                    target=ActionTarget.DESKTOP(display_id="primary"),
                    scope=None,
                    session=None,
                )
            )
        )

    async def press_key(self, key: str) -> dict[str, object]:
        return await self._mutate_then_observe(
            lambda: self.driver.press_key(
                PressKeyInput(
                    key=key,
                    modifiers=None,
                    target=ActionTarget.DESKTOP(display_id="primary"),
                    scope=None,
                    session=None,
                )
            )
        )

    async def _mutate_then_observe(
        self,
        operation: Callable[[], Awaitable[ToolResult]],
    ) -> dict[str, object]:
        unknown_detail: str | None = None
        try:
            result = await self._bounded(operation())
            if result.is_error:
                unknown_detail = (
                    f"Action reported an error and its outcome may be unknown "
                    f"({result.text}). A fresh observation follows. Do not retry "
                    "until the observation proves the action did not land."
                )
        except Exception as error:
            unknown_detail = (
                f"Action outcome is unknown ({error}). A fresh observation follows. "
                "Do not retry until the observation proves the action did not land."
            )

        observation = await self.observe()
        if unknown_detail:
            observation["content"] = [
                {"type": "text", "text": unknown_detail},
                *observation["content"],
            ]
        return observation

    async def _bounded(self, operation: Awaitable[T]) -> T:
        return await asyncio.wait_for(operation, self.timeout)

    @staticmethod
    def _content(result: ToolResult) -> dict[str, object]:
        content: list[ToolContent] = [{"type": "text", "text": result.text}]
        content.extend(
            {
                "type": "image",
                "data": image.data_base64,
                "mimeType": image.mime_type,
            }
            for image in result.images
        )
        return {"content": content, "isError": result.is_error}
