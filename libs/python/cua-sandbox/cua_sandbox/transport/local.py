"""LocalTransport — delegates to cua_auto for direct host control.

All cua_auto calls are synchronous; we use asyncio.to_thread() to avoid
blocking the event loop.
"""

from __future__ import annotations

import asyncio
import platform
from typing import Any, Dict

from cua_sandbox.transport.base import Transport


class LocalTransport(Transport):
    """Transport that routes commands directly to cua_auto modules."""

    async def connect(self) -> None:
        pass  # no-op for local

    async def disconnect(self) -> None:
        pass  # no-op for local

    async def send(self, action: str, **params: Any) -> Any:
        """Dispatch a named command to the appropriate cua_auto module."""
        return await asyncio.to_thread(self._dispatch, action, params)

    def _dispatch(self, command: str, params: dict) -> Any:
        import cua_auto.clipboard as clipboard
        import cua_auto.keyboard as keyboard
        import cua_auto.mouse as mouse
        import cua_auto.shell as shell
        import cua_auto.window as window

        dispatch = {
            # Mouse
            "left_click": lambda p: mouse.click(p["x"], p["y"], p.get("button", "left")),
            "right_click": lambda p: mouse.right_click(p["x"], p["y"]),
            "double_click": lambda p: mouse.double_click(p["x"], p["y"]),
            "move_cursor": lambda p: mouse.move_to(p["x"], p["y"]),
            "scroll": lambda p: (
                mouse.move_to(p.get("x", 0), p.get("y", 0)),
                mouse.scroll(p.get("scroll_x", 0), p.get("scroll_y", 3)),
            ),
            "mouse_down": lambda p: mouse.mouse_down(
                p.get("x"), p.get("y"), p.get("button", "left")
            ),
            "mouse_up": lambda p: mouse.mouse_up(p.get("x"), p.get("y"), p.get("button", "left")),
            "drag": lambda p: mouse.drag(
                p["start_x"], p["start_y"], p["end_x"], p["end_y"], p.get("button", "left")
            ),
            # Keyboard
            "type_text": lambda p: keyboard.type_text(p["text"]),
            "hotkey": lambda p: keyboard.hotkey(
                p["keys"] if isinstance(p["keys"], list) else [p["keys"]]
            ),
            "key_down": lambda p: keyboard.key_down(p["key"]),
            "key_up": lambda p: keyboard.key_up(p["key"]),
            # Clipboard
            "copy_to_clipboard": lambda p: clipboard.get(),
            "set_clipboard": lambda p: clipboard.set(p["text"]),
            # Shell
            "run_command": lambda p: shell.run(p["command"], p.get("timeout", 30)),
            # Window
            "get_active_window_title": lambda p: window.get_active_window_title(),
        }

        handler = dispatch.get(command)
        if handler is None:
            raise ValueError(f"Unknown local command: {command}")
        return handler(params)

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        png = await asyncio.to_thread(self._screenshot_sync)
        from cua_sandbox.transport.base import convert_screenshot

        return convert_screenshot(png, format, quality)

    def _screenshot_sync(self) -> bytes:
        import cua_auto.screen as screen

        return screen.screenshot_bytes()

    async def get_screen_size(self) -> Dict[str, int]:
        return await asyncio.to_thread(self._screen_size_sync)

    def _screen_size_sync(self) -> Dict[str, int]:
        import cua_auto.screen as screen

        w, h = screen.screen_size()
        return {"width": w, "height": h}

    async def get_environment(self) -> str:
        system = platform.system()
        if system == "Darwin":
            return "mac"
        if system == "Windows":
            return "windows"
        return "linux"
