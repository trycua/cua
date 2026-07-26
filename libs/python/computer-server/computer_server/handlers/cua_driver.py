"""Cua Driver-backed automation compatibility handler.

The compatibility server keeps its remote HTTP/WebSocket, shell, file, PTY,
and authentication surfaces while delegating the portable desktop action space
to the generated ``cua-driver`` Python SDK. Operations that are not part of the
portable driver contract remain on the supplied legacy handler until they have
an equivalent Rust implementation.
"""

from __future__ import annotations

import asyncio
import base64
import importlib
import json
import os
import uuid
from io import BytesIO
from types import ModuleType
from typing import Any, Dict, List, Optional, Protocol, Tuple

from PIL import Image

from .base import BaseAutomationHandler, normalize_screenshot_format


class DriverImage(Protocol):
    mime_type: str
    data_base64: str


class DriverResult(Protocol):
    structured_json: Optional[str]
    images: List[DriverImage]
    is_error: bool
    text: str
    error_code: Optional[str]
    verified: Optional[bool]
    degraded: bool


class DriverClient(Protocol):
    async def start_session(self, input: Any) -> Any: ...

    async def end_session(self, input: Any) -> Any: ...

    async def escalate_session(self, input: Any) -> Any: ...

    async def get_session_state(self, input: Any) -> Any: ...

    async def get_desktop_state(self, input: Any) -> DriverResult: ...

    async def get_screen_size(self, input: Any) -> DriverResult: ...

    async def get_cursor_position(self, input: Any) -> DriverResult: ...

    async def move_cursor(self, input: Any) -> DriverResult: ...

    async def click(self, input: Any) -> DriverResult: ...

    async def drag(self, input: Any) -> DriverResult: ...

    async def scroll(self, input: Any) -> DriverResult: ...

    async def type_text(self, input: Any) -> DriverResult: ...

    async def press_key(self, input: Any) -> DriverResult: ...

    async def hotkey(self, input: Any) -> DriverResult: ...

    async def shutdown(self) -> None: ...


class CuaDriverAutomationHandler(BaseAutomationHandler):
    """Translate the legacy automation API to the generated typed SDK.

    ``embedded`` is the default because computer-server already owns a
    long-lived process and desktop permission identity. ``daemon`` preserves a
    compatibility option for deployments that intentionally centralize the
    driver in a separate stable process.
    """

    def __init__(
        self,
        fallback: BaseAutomationHandler,
        *,
        driver: Optional[DriverClient] = None,
        sdk: Optional[ModuleType] = None,
        mode: Optional[str] = None,
        socket_path: Optional[str] = None,
        session_id: Optional[str] = None,
        capture_scope: Optional[str] = None,
    ) -> None:
        self._sdk = sdk or self._load_sdk()
        selected_mode = (mode or os.environ.get("CUA_DRIVER_MODE", "embedded")).strip()
        if selected_mode == "in-process":
            selected_mode = "embedded"
        if selected_mode not in {"embedded", "daemon"}:
            raise ValueError("CUA_DRIVER_MODE must be embedded or daemon")

        if driver is None:
            if selected_mode == "embedded":
                driver = self._sdk.CuaDriver.create()
            else:
                driver = self._sdk.CuaDriver.connect(
                    socket_path or os.environ.get("CUA_DRIVER_SOCKET")
                )

        self._driver = driver
        self._fallback = fallback
        self._mode = selected_mode
        self._session_id = (
            session_id
            or os.environ.get("CUA_DRIVER_SESSION_ID")
            or (f"computer-server-{os.getpid()}-{uuid.uuid4().hex[:8]}")
        )
        selected_scope = (
            (capture_scope or os.environ.get("CUA_DRIVER_CAPTURE_SCOPE", "desktop")).strip().lower()
        )
        if selected_scope not in {"auto", "window", "desktop"}:
            raise ValueError("CUA_DRIVER_CAPTURE_SCOPE must be auto, window, or desktop")
        self._capture_scope = selected_scope
        self._session_started = False
        self._session_lock = asyncio.Lock()
        self._closed = False

    @staticmethod
    def _load_sdk() -> ModuleType:
        try:
            return importlib.import_module("cua_driver")
        except ImportError as error:  # pragma: no cover - package-install path
            raise RuntimeError(
                "CUA_BACKEND=cua-driver requires cua-computer-server[driver]"
            ) from error

    @property
    def mode(self) -> str:
        return self._mode

    @property
    def capture_scope(self) -> str:
        return self._capture_scope

    async def _ensure_session(self) -> None:
        if self._closed:
            raise RuntimeError("Cua Driver automation handler is closed")
        if self._session_started:
            return
        async with self._session_lock:
            if self._session_started:
                return
            started = await self._driver.start_session(
                self._sdk.StartSessionInput(
                    session=self._session_id,
                    capture_scope={
                        "auto": self._sdk.CaptureScope.AUTO,
                        "window": self._sdk.CaptureScope.WINDOW,
                        "desktop": self._sdk.CaptureScope.DESKTOP,
                    }[self._capture_scope],
                )
            )
            if not started.active:
                raise RuntimeError("Cua Driver did not activate the compatibility session")
            self._session_started = True

    @staticmethod
    def _structured(result: DriverResult) -> Dict[str, Any]:
        if not result.structured_json:
            return {}
        value = json.loads(result.structured_json)
        if not isinstance(value, dict):
            raise ValueError("Cua Driver structured result must be an object")
        return value

    @staticmethod
    def _raise_for_error(result: DriverResult) -> None:
        if not result.is_error:
            return
        detail = result.text or "Cua Driver tool failed"
        if result.error_code:
            detail = f"{detail} ({result.error_code})"
        raise RuntimeError(detail)

    @classmethod
    def _result_data(cls, result: DriverResult) -> Dict[str, Any]:
        cls._raise_for_error(result)
        data = cls._structured(result)
        if result.verified is not None:
            data.setdefault("verified", result.verified)
        if result.degraded:
            data.setdefault("degraded", True)
        return data

    async def _point(self, x: Optional[int], y: Optional[int]) -> tuple[int, int]:
        if x is not None and y is not None:
            return int(x), int(y)
        position = await self.get_cursor_position()
        if not position.get("success"):
            raise RuntimeError(position.get("error", "Cursor position is unavailable"))
        return int(position["position"]["x"]), int(position["position"]["y"])

    @staticmethod
    def _enum_name(value: Any) -> Optional[str]:
        if value is None:
            return None
        return str(value.name).lower()

    @classmethod
    def _session_state(cls, state: Any) -> Dict[str, Any]:
        return {
            "session": state.session,
            "capture_scope": cls._enum_name(state.capture_scope),
            "effective_scope": cls._enum_name(state.effective_scope),
            "desktop_unlocked": state.desktop_unlocked,
            "escalation_reason": cls._enum_name(state.escalation_reason),
            "escalation_detail": state.escalation_detail,
        }

    @staticmethod
    def _ok(data: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        return {"success": True, **(data or {})}

    @staticmethod
    def _error(error: Exception) -> Dict[str, Any]:
        return {"success": False, "error": str(error)}

    async def close(self) -> None:
        """End the compatibility session and release the SDK runtime once."""

        async with self._session_lock:
            if self._closed:
                return
            try:
                if self._session_started:
                    await self._driver.end_session(
                        self._sdk.EndSessionInput(session=self._session_id)
                    )
                    self._session_started = False
            finally:
                await self._driver.shutdown()
                self._closed = True

    async def get_capture_scope_state(self) -> Dict[str, Any]:
        """Return the generated session policy state in the legacy envelope."""

        try:
            await self._ensure_session()
            state = await self._driver.get_session_state(
                self._sdk.GetSessionStateInput(session=self._session_id)
            )
            return self._ok(self._session_state(state))
        except Exception as error:
            return self._error(error)

    async def escalate_capture_scope(
        self, reason: str, detail: Optional[str] = None
    ) -> Dict[str, Any]:
        """Explicitly unlock desktop scope for an ``auto`` session.

        The caller—not computer-server—decides when the window-focused ladder
        is exhausted. Strict ``window`` sessions remain non-escalatable.
        """

        normalized = reason.strip().lower()
        reasons = {
            "ax_tree_pixel_mismatch": self._sdk.EscalationReason.AX_TREE_PIXEL_MISMATCH,
            "background_delivery_failed": self._sdk.EscalationReason.BACKGROUND_DELIVERY_FAILED,
            "foreground_ineffective": self._sdk.EscalationReason.FOREGROUND_INEFFECTIVE,
            "no_window_target": self._sdk.EscalationReason.NO_WINDOW_TARGET,
            "other": self._sdk.EscalationReason.OTHER,
        }
        if normalized not in reasons:
            return self._error(
                ValueError(
                    "reason must be ax_tree_pixel_mismatch, "
                    "background_delivery_failed, foreground_ineffective, "
                    "no_window_target, or other"
                )
            )
        try:
            await self._ensure_session()
            state = await self._driver.escalate_session(
                self._sdk.EscalateSessionInput(
                    session=self._session_id,
                    reason=reasons[normalized],
                    detail=detail,
                )
            )
            return self._ok(self._session_state(state))
        except Exception as error:
            return self._error(error)

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
            await self._ensure_session()
            px, py = await self._point(x, y)
            result = await self._driver.click(
                self._sdk.ClickInput(
                    x=float(px),
                    y=float(py),
                    scope=self._sdk.DesktopScope.DESKTOP,
                    session=self._session_id,
                    button={
                        "left": self._sdk.ClickButton.LEFT,
                        "right": self._sdk.ClickButton.RIGHT,
                        "middle": self._sdk.ClickButton.MIDDLE,
                    }[button],
                    count=count,
                )
            )
            return self._ok(self._result_data(result))
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
            await self._ensure_session()
            result = await self._driver.move_cursor(
                self._sdk.MoveCursorInput(
                    x=float(x),
                    y=float(y),
                    scope=self._sdk.DesktopScope.DESKTOP,
                    session=self._session_id,
                )
            )
            return self._ok(self._result_data(result))
        except Exception as error:
            return self._error(error)

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        try:
            start_x, start_y = await self._point(None, None)
            return await self._drag_points(
                start_x, start_y, x, y, button=button, duration=duration, steps=None
            )
        except Exception as error:
            return self._error(error)

    async def _drag_points(
        self,
        start_x: int,
        start_y: int,
        end_x: int,
        end_y: int,
        *,
        button: str,
        duration: float,
        steps: Optional[int],
    ) -> Dict[str, Any]:
        await self._ensure_session()
        result = await self._driver.drag(
            self._sdk.DragInput(
                from_x=float(start_x),
                from_y=float(start_y),
                to_x=float(end_x),
                to_y=float(end_y),
                scope=self._sdk.DesktopScope.DESKTOP,
                session=self._session_id,
                duration_ms=min(10000, max(0, int(duration * 1000))),
                steps=steps,
                button={
                    "left": self._sdk.ClickButton.LEFT,
                    "right": self._sdk.ClickButton.RIGHT,
                    "middle": self._sdk.ClickButton.MIDDLE,
                }[button],
                modifier=None,
            )
        )
        return self._ok(self._result_data(result))

    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        if len(path) < 2:
            return {"success": False, "error": "Drag path requires at least two points"}
        try:
            start_x, start_y = path[0]
            end_x, end_y = path[-1]
            return await self._drag_points(
                start_x,
                start_y,
                end_x,
                end_y,
                button=button,
                duration=duration,
                steps=min(200, max(1, len(path) - 1)),
            )
        except Exception as error:
            return self._error(error)

    async def key_down(self, key: str) -> Dict[str, Any]:
        return await self._fallback.key_down(key)

    async def key_up(self, key: str) -> Dict[str, Any]:
        return await self._fallback.key_up(key)

    async def type_text(self, text: str) -> Dict[str, Any]:
        try:
            await self._ensure_session()
            result = await self._driver.type_text(
                self._sdk.TypeTextInput(
                    text=text,
                    scope=self._sdk.DesktopScope.DESKTOP,
                    session=self._session_id,
                )
            )
            return self._ok(self._result_data(result))
        except Exception as error:
            return self._error(error)

    async def press_key(self, key: str) -> Dict[str, Any]:
        try:
            await self._ensure_session()
            result = await self._driver.press_key(
                self._sdk.PressKeyInput(
                    key=key,
                    scope=self._sdk.DesktopScope.DESKTOP,
                    session=self._session_id,
                    modifiers=None,
                )
            )
            return self._ok(self._result_data(result))
        except Exception as error:
            return self._error(error)

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        if not keys:
            return {"success": False, "error": "Hotkey requires at least one key"}
        if len(keys) == 1:
            return await self.press_key(keys[0])
        try:
            await self._ensure_session()
            result = await self._driver.hotkey(
                self._sdk.HotkeyInput(
                    keys=keys,
                    scope=self._sdk.DesktopScope.DESKTOP,
                    session=self._session_id,
                )
            )
            return self._ok(self._result_data(result))
        except Exception as error:
            return self._error(error)

    async def _scroll(self, direction: str, amount: int) -> Dict[str, Any]:
        try:
            await self._ensure_session()
            x, y = await self._point(None, None)
            result = await self._driver.scroll(
                self._sdk.ScrollInput(
                    x=float(x),
                    y=float(y),
                    direction={
                        "up": self._sdk.ScrollDirection.UP,
                        "down": self._sdk.ScrollDirection.DOWN,
                        "left": self._sdk.ScrollDirection.LEFT,
                        "right": self._sdk.ScrollDirection.RIGHT,
                    }[direction],
                    scope=self._sdk.DesktopScope.DESKTOP,
                    session=self._session_id,
                    by=self._sdk.ScrollBy.LINE,
                    amount=min(50, max(1, amount)),
                )
            )
            return self._ok(self._result_data(result))
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

    async def get_desktop_state(self, screenshot_out_file: Optional[str] = None) -> Dict[str, Any]:
        """Return the canonical whole-desktop capture in the legacy envelope."""

        try:
            await self._ensure_session()
            result = await self._driver.get_desktop_state(
                self._sdk.GetDesktopStateInput(
                    session=self._session_id,
                    screenshot_out_file=screenshot_out_file,
                )
            )
            data = self._result_data(result)
            images = [
                {"mime_type": image.mime_type, "data_base64": image.data_base64}
                for image in result.images
            ]
            if images:
                data.setdefault("image_data", images[0]["data_base64"])
                data.setdefault("format", images[0]["mime_type"].split("/", 1)[-1])
            return self._ok({**data, "images": images})
        except Exception as error:
            return self._error(error)

    async def screenshot(self, format: str = "png", quality: int = 95) -> Dict[str, Any]:
        try:
            fmt, quality = normalize_screenshot_format(format, quality)
            desktop = await self.get_desktop_state()
            if not desktop.get("success"):
                raise RuntimeError(desktop.get("error", "Desktop capture failed"))
            image_data = desktop.get("image_data")
            if not image_data:
                raise RuntimeError("Cua Driver get_desktop_state returned no image")
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
            await self._ensure_session()
            result = await self._driver.get_screen_size(
                self._sdk.GetScreenSizeInput(session=self._session_id)
            )
            data = self._result_data(result)
            return self._ok({"size": {"width": int(data["width"]), "height": int(data["height"])}})
        except Exception as error:
            return self._error(error)

    async def get_cursor_position(self) -> Dict[str, Any]:
        try:
            await self._ensure_session()
            result = await self._driver.get_cursor_position(
                self._sdk.GetCursorPositionInput(session=self._session_id)
            )
            data = self._result_data(result)
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
