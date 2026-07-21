"""Cua Driver-backed automation compatibility handler.

This adapter keeps computer-server's remote HTTP/WebSocket protocol while
delegating the overlapping desktop operations to the local Rust daemon through
the Python ``cua-driver`` SDK. Operations the driver does not expose (notably
held key/button state, clipboard, and shell) remain on the supplied legacy
handler so the compatibility server does not silently lose capabilities.
"""

from __future__ import annotations

import asyncio
import base64
import json
import os
from io import BytesIO
from typing import Any, Dict, List, Optional, Protocol, Tuple

from PIL import Image

from .base import BaseAutomationHandler, normalize_screenshot_format


class DriverResult(Protocol):
    """Structural subset of ``cua_driver.ToolResult`` used by the adapter."""

    structured_json: Optional[str]
    images: List[Any]
    is_error: bool
    text: str
    error_code: Optional[str]


class DriverClient(Protocol):
    def call_tool(self, name: str, arguments_json: str) -> DriverResult: ...


class CuaDriverAutomationHandler(BaseAutomationHandler):
    """Translate the legacy automation handler API to Cua Driver tool calls.

    The driver SDK is synchronous because it crosses UniFFI into a local daemon
    socket. Calls run in worker threads so FastAPI's event loop remains usable.
    A desktop-scope session is declared lazily to preserve computer-server's
    full-desktop behavior.
    """

    def __init__(
        self,
        fallback: BaseAutomationHandler,
        *,
        driver: Optional[DriverClient] = None,
        socket_path: Optional[str] = None,
        session_id: Optional[str] = None,
    ) -> None:
        if driver is None:
            try:
                from cua_driver import CuaDriver
            except ImportError as error:  # pragma: no cover - exercised by package installs
                raise RuntimeError(
                    "CUA_BACKEND=cua-driver requires cua-computer-server[driver]"
                ) from error
            driver = CuaDriver.connect(socket_path or os.environ.get("CUA_DRIVER_SOCKET"))

        self._driver = driver
        self._fallback = fallback
        self._session_id = session_id or os.environ.get(
            "CUA_DRIVER_SESSION_ID", "computer-server-compat"
        )
        self._session_started = False
        self._session_lock = asyncio.Lock()

    async def _raw_call(self, name: str, arguments: Dict[str, Any]) -> DriverResult:
        return await asyncio.to_thread(self._driver.call_tool, name, json.dumps(arguments))

    async def _ensure_session(self) -> None:
        if self._session_started:
            return
        async with self._session_lock:
            if self._session_started:
                return
            result = await self._raw_call(
                "start_session",
                {"session": self._session_id, "capture_scope": "desktop"},
            )
            if result.is_error:
                raise RuntimeError(result.text or "Cua Driver rejected start_session")
            self._session_started = True

    @staticmethod
    def _structured(result: DriverResult) -> Dict[str, Any]:
        if not result.structured_json:
            return {}
        value = json.loads(result.structured_json)
        if not isinstance(value, dict):
            raise ValueError("Cua Driver structured result must be an object")
        return value

    async def _call(
        self, name: str, arguments: Dict[str, Any]
    ) -> tuple[DriverResult, Dict[str, Any]]:
        await self._ensure_session()
        args = {**arguments, "session": self._session_id}
        result = await self._raw_call(name, args)
        if result.is_error:
            detail = result.text or "Cua Driver tool failed"
            if result.error_code:
                detail = f"{detail} ({result.error_code})"
            raise RuntimeError(detail)
        return result, self._structured(result)

    async def _point(self, x: Optional[int], y: Optional[int]) -> tuple[int, int]:
        if x is not None and y is not None:
            return int(x), int(y)
        _, data = await self._call("get_cursor_position", {})
        if not data.get("available", True):
            raise RuntimeError("Cua Driver cannot observe the current cursor position")
        return int(data["x"]), int(data["y"])

    @staticmethod
    def _ok(data: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        return {"success": True, **(data or {})}

    @staticmethod
    def _error(error: Exception) -> Dict[str, Any]:
        return {"success": False, "error": str(error)}

    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        return await self._fallback.mouse_down(x, y, button)

    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        return await self._fallback.mouse_up(x, y, button)

    async def _click(
        self, x: Optional[int], y: Optional[int], *, button: str, count: int
    ) -> Dict[str, Any]:
        try:
            px, py = await self._point(x, y)
            _, data = await self._call(
                "click",
                {
                    "x": px,
                    "y": py,
                    "scope": "desktop",
                    "button": button,
                    "count": count,
                },
            )
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        return await self._click(x, y, button="left", count=1)

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        return await self._click(x, y, button="right", count=1)

    async def middle_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        return await self._click(x, y, button="middle", count=1)

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        return await self._click(x, y, button="left", count=2)

    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        try:
            _, data = await self._call("move_cursor", {"x": x, "y": y, "scope": "desktop"})
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        try:
            start_x, start_y = await self._point(None, None)
            _, data = await self._call(
                "drag",
                {
                    "from_x": start_x,
                    "from_y": start_y,
                    "to_x": x,
                    "to_y": y,
                    "scope": "desktop",
                    "button": button,
                    "duration_ms": min(10000, max(0, int(duration * 1000))),
                },
            )
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        if len(path) < 2:
            return {"success": False, "error": "Drag path requires at least two points"}
        try:
            start_x, start_y = path[0]
            end_x, end_y = path[-1]
            _, data = await self._call(
                "drag",
                {
                    "from_x": start_x,
                    "from_y": start_y,
                    "to_x": end_x,
                    "to_y": end_y,
                    "scope": "desktop",
                    "button": button,
                    "duration_ms": min(10000, max(0, int(duration * 1000))),
                    "steps": min(200, max(1, len(path) - 1)),
                },
            )
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def key_down(self, key: str) -> Dict[str, Any]:
        return await self._fallback.key_down(key)

    async def key_up(self, key: str) -> Dict[str, Any]:
        return await self._fallback.key_up(key)

    async def type_text(self, text: str) -> Dict[str, Any]:
        try:
            _, data = await self._call("type_text", {"text": text, "scope": "desktop"})
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def press_key(self, key: str) -> Dict[str, Any]:
        try:
            _, data = await self._call(
                "press_key",
                {"key": key, "scope": "desktop", "modifiers": []},
            )
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        if not keys:
            return {"success": False, "error": "Hotkey requires at least one key"}
        if len(keys) == 1:
            return await self.press_key(keys[0])
        try:
            _, data = await self._call("hotkey", {"keys": keys, "scope": "desktop"})
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def _scroll(self, direction: str, amount: int) -> Dict[str, Any]:
        try:
            x, y = await self._point(None, None)
            _, data = await self._call(
                "scroll",
                {
                    "x": x,
                    "y": y,
                    "direction": direction,
                    "scope": "desktop",
                    "by": "line",
                    "amount": min(50, max(1, amount)),
                },
            )
            return self._ok(data)
        except Exception as error:
            return self._error(error)

    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        if y:
            return await self._scroll("up" if y > 0 else "down", abs(y))
        if x:
            return await self._scroll("right" if x > 0 else "left", abs(x))
        return {"success": False, "error": "Scroll amount must be non-zero"}

    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        return await self._scroll("down", abs(clicks))

    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        return await self._scroll("up", abs(clicks))

    async def screenshot(self, format: str = "png", quality: int = 95) -> Dict[str, Any]:
        try:
            fmt, quality = normalize_screenshot_format(format, quality)
            result, _ = await self._call("get_desktop_state", {})
            if not result.images:
                raise RuntimeError("Cua Driver get_desktop_state returned no image")
            image_data = result.images[0].data_base64
            if fmt == "jpeg":
                raw = base64.b64decode(image_data)

                def convert() -> str:
                    with Image.open(BytesIO(raw)) as image:
                        output = BytesIO()
                        image.convert("RGB").save(
                            output, format="JPEG", quality=quality, optimize=True
                        )
                        return base64.b64encode(output.getvalue()).decode("ascii")

                image_data = await asyncio.to_thread(convert)
            return self._ok({"image_data": image_data, "format": fmt})
        except Exception as error:
            return self._error(error)

    async def get_screen_size(self) -> Dict[str, Any]:
        try:
            _, data = await self._call("get_screen_size", {})
            return self._ok({"size": {"width": int(data["width"]), "height": int(data["height"])}})
        except Exception as error:
            return self._error(error)

    async def get_cursor_position(self) -> Dict[str, Any]:
        try:
            _, data = await self._call("get_cursor_position", {})
            if not data.get("available", True):
                raise RuntimeError("Cua Driver cannot observe the current cursor position")
            return self._ok({"position": {"x": int(data["x"]), "y": int(data["y"])}})
        except Exception as error:
            return self._error(error)

    async def copy_to_clipboard(self) -> Dict[str, Any]:
        return await self._fallback.copy_to_clipboard()

    async def set_clipboard(self, text: str) -> Dict[str, Any]:
        return await self._fallback.set_clipboard(text)

    async def run_command(self, command: str, timeout: Optional[float] = None) -> Dict[str, Any]:
        return await self._fallback.run_command(command, timeout)
