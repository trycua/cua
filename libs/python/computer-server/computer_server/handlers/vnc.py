"""
VNC backend for automation and accessibility handlers.

A cross-platform alternative to the native (OS-specific) handlers. Connects to
any VNC server to perform screenshots, mouse, keyboard, and scroll operations
over the RFB protocol. Works with Linux, macOS, and Windows targets — the only
requirement is a reachable VNC server.

Built on vncdotool (Twisted-based RFB client). The Twisted reactor runs in a
background daemon thread; all handler methods bridge from asyncio via
`asyncio.to_thread`.

Usage:
    # Via CLI
    python -m computer_server --vnc-host 192.168.64.1 --vnc-port 5900 --vnc-password secret

    # Via env vars
    CUA_VNC_HOST=192.168.64.1 CUA_VNC_PORT=5900 CUA_VNC_PASSWORD=secret python -m computer_server
"""

import asyncio
import base64
import logging
import threading
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

from .base import BaseAccessibilityHandler, BaseAutomationHandler

logger = logging.getLogger(__name__)

# Key name aliases: map cua-computer-server key names → vncdotool key names.
# vncdotool's KEYMAP already covers most X11 keysym names; these bridge
# the naming differences used by the rest of the computer-server API.
_KEY_ALIASES = {
    "return": "enter",
    "escape": "esc",
    "backspace": "bsp",
    "delete": "del",
    "page_up": "pgup",
    "pageup": "pgup",
    "page_down": "pgdn",
    "pagedown": "pgdn",
    "insert": "ins",
    "caps_lock": "caplk",
    "num_lock": "numlk",
    "shift_l": "lshift",
    "shift_r": "rshift",
    "ctrl_l": "lctrl",
    "ctrl_r": "rctrl",
    "control": "ctrl",
    "control_l": "lctrl",
    "control_r": "rctrl",
    "alt_l": "lalt",
    "alt_r": "ralt",
    "meta_l": "lmeta",
    "meta_r": "rmeta",
    "super_l": "lsuper",
    "super_r": "rsuper",
    # macOS-specific names
    "command": "alt",
    "cmd": "alt",
    "option": "meta",
    "option_l": "lmeta",
    "option_r": "rmeta",
}

# Button name → vncdotool button number (1-indexed)
_BUTTON_MAP = {"left": 1, "middle": 2, "right": 3}


def _translate_key(key: str) -> str:
    """Translate a key name to vncdotool's KEYMAP name."""
    lower = key.lower()
    return _KEY_ALIASES.get(lower, lower)


# ---------------------------------------------------------------------------
# Lazy vncdotool client wrapper
# ---------------------------------------------------------------------------


class _VNCConnection:
    """Manages a vncdotool ThreadedVNCClientProxy lifecycle.

    All methods are synchronous (blocking) — the handler calls them via
    asyncio.to_thread to avoid blocking the event loop.
    """

    def __init__(self, host: str, port: int, password: str = ""):
        self._host = host
        self._port = port
        self._password = password
        self._client = None
        self._lock = threading.Lock()
        # Track cursor position locally (vncdotool also tracks via client.x/y)
        self._cursor_x: int = 0
        self._cursor_y: int = 0

    def _ensure_connected(self):
        """Lazily connect to the VNC server. Returns the vncdotool client."""
        if self._client is not None:
            return self._client
        with self._lock:
            if self._client is not None:
                return self._client
            from vncdotool import api

            server_str = f"{self._host}::{self._port}"
            logger.info(f"VNC connecting to {self._host}:{self._port}")
            client = api.connect(
                server_str,
                password=self._password or None,
            )
            client.timeout = 30
            self._client = client
            logger.info("VNC connected")
            return client

    def disconnect(self):
        with self._lock:
            if self._client is not None:
                try:
                    self._client.disconnect()
                except Exception:
                    pass
                self._client = None

    def _reset_on_error(self):
        """Disconnect so next call reconnects."""
        try:
            self.disconnect()
        except Exception:
            pass

    # -- Screenshot ---------------------------------------------------------

    def capture_screenshot(self) -> bytes:
        """Capture the screen and return PNG bytes."""
        client = self._ensure_connected()
        # refreshScreen updates the internal screen image
        client.refreshScreen(incremental=False)
        screen = client.protocol.screen
        buf = BytesIO()
        screen.save(buf, format="PNG")
        return buf.getvalue()

    # -- Mouse --------------------------------------------------------------

    def mouse_move(self, x: int, y: int):
        client = self._ensure_connected()
        client.mouseMove(x, y)
        self._cursor_x = x
        self._cursor_y = y

    def mouse_click(self, x: int, y: int, button: int = 1, clicks: int = 1):
        client = self._ensure_connected()
        client.mouseMove(x, y)
        self._cursor_x = x
        self._cursor_y = y
        for _ in range(clicks):
            client.mousePress(button)

    def mouse_down(self, x: int, y: int, button: int = 1):
        client = self._ensure_connected()
        client.mouseMove(x, y)
        self._cursor_x = x
        self._cursor_y = y
        client.mouseDown(button)

    def mouse_up(self, x: int, y: int, button: int = 1):
        client = self._ensure_connected()
        client.mouseMove(x, y)
        self._cursor_x = x
        self._cursor_y = y
        client.mouseUp(button)

    def mouse_drag(self, x: int, y: int, step: int = 1):
        client = self._ensure_connected()
        client.mouseDrag(x, y, step=step)
        self._cursor_x = x
        self._cursor_y = y

    def scroll(self, x: int, y: int):
        """Scroll. y>0 = down (button 5), y<0 = up (button 4)."""
        client = self._ensure_connected()
        if y > 0:
            for _ in range(y):
                client.mousePress(5)
        elif y < 0:
            for _ in range(abs(y)):
                client.mousePress(4)
        # Horizontal: button 6 (right), button 7 (left) — not universally supported
        if x > 0:
            for _ in range(x):
                client.mousePress(6)
        elif x < 0:
            for _ in range(abs(x)):
                client.mousePress(7)

    # -- Keyboard -----------------------------------------------------------

    def key_press(self, key: str):
        client = self._ensure_connected()
        client.keyPress(_translate_key(key))

    def key_down(self, key: str):
        client = self._ensure_connected()
        client.keyDown(_translate_key(key))

    def key_up(self, key: str):
        client = self._ensure_connected()
        client.keyUp(_translate_key(key))

    def type_text(self, text: str):
        """Type text character by character, handling shift for uppercase/symbols."""
        client = self._ensure_connected()
        for ch in text:
            if ch == " ":
                client.keyPress("space")
            elif ch == "\n":
                client.keyPress("enter")
            elif ch == "\t":
                client.keyPress("tab")
            else:
                # vncdotool's keyPress handles single characters directly
                client.keyPress(ch)

    def hotkey(self, keys: List[str]):
        """Press a key combination (e.g. ['command', 'a'])."""
        client = self._ensure_connected()
        translated = [_translate_key(k) for k in keys]
        # vncdotool supports "modifier-key" syntax: "alt-a", "ctrl-shift-c"
        combo = "-".join(translated)
        client.keyPress(combo)

    # -- Clipboard ----------------------------------------------------------

    def paste_text(self, text: str):
        client = self._ensure_connected()
        client.paste(text)

    # -- Info ---------------------------------------------------------------

    @property
    def screen_size(self) -> Tuple[int, int]:
        client = self._ensure_connected()
        proto = client.protocol
        if proto is not None and proto.screen is not None:
            return proto.screen.size
        return (0, 0)

    @property
    def cursor_position(self) -> Tuple[int, int]:
        return self._cursor_x, self._cursor_y


# ---------------------------------------------------------------------------
# Automation Handler
# ---------------------------------------------------------------------------


class VNCAutomationHandler(BaseAutomationHandler):
    """Cross-platform automation handler that operates via VNC/RFB protocol.

    Works with any OS target (Linux, macOS, Windows) that has a VNC server.
    All operations go through the network, bypassing local permission systems.
    """

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 5900,
        password: str = "",
    ):
        self._conn = _VNCConnection(host, port, password)

    def _resolve_coords(self, x: Optional[int], y: Optional[int]) -> Tuple[int, int]:
        if x is not None and y is not None:
            return x, y
        return self._conn.cursor_position

    # -- Screenshot ---------------------------------------------------------

    async def screenshot(self) -> Dict[str, Any]:
        try:
            png_bytes = await asyncio.to_thread(self._conn.capture_screenshot)
            image_data = base64.b64encode(png_bytes).decode()
            return {"success": True, "image_data": image_data}
        except Exception as e:
            logger.error(f"VNC screenshot error: {e}")
            self._conn._reset_on_error()
            return {"success": False, "error": f"VNC screenshot error: {e}"}

    # -- Mouse actions ------------------------------------------------------

    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            cx, cy = self._resolve_coords(x, y)
            await asyncio.to_thread(self._conn.mouse_click, cx, cy, 1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            cx, cy = self._resolve_coords(x, y)
            await asyncio.to_thread(self._conn.mouse_click, cx, cy, 3)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def middle_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            cx, cy = self._resolve_coords(x, y)
            await asyncio.to_thread(self._conn.mouse_click, cx, cy, 2)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def double_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            cx, cy = self._resolve_coords(x, y)
            await asyncio.to_thread(self._conn.mouse_click, cx, cy, 1, 2)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        try:
            await asyncio.to_thread(self._conn.mouse_move, x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        try:
            cx, cy = self._resolve_coords(x, y)
            btn = _BUTTON_MAP.get(button, 1)
            await asyncio.to_thread(self._conn.mouse_down, cx, cy, btn)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        try:
            cx, cy = self._resolve_coords(x, y)
            btn = _BUTTON_MAP.get(button, 1)
            await asyncio.to_thread(self._conn.mouse_up, cx, cy, btn)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        try:
            btn = _BUTTON_MAP.get(button, 1)
            cx, cy = self._conn.cursor_position

            def _drag():
                self._conn.mouse_down(cx, cy, btn)
                step = max(1, int(1 / max(duration, 0.01) * 5))
                self._conn.mouse_drag(x, y, step=step)
                self._conn.mouse_up(x, y, btn)

            await asyncio.to_thread(_drag)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        try:
            if not path:
                return {"success": False, "error": "Empty path"}
            btn = _BUTTON_MAP.get(button, 1)

            def _drag_path():
                self._conn.mouse_down(path[0][0], path[0][1], btn)
                for px, py in path[1:]:
                    self._conn.mouse_move(px, py)
                self._conn.mouse_up(path[-1][0], path[-1][1], btn)

            await asyncio.to_thread(_drag_path)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # -- Keyboard actions ---------------------------------------------------

    async def type_text(self, text: str) -> Dict[str, Any]:
        try:
            await asyncio.to_thread(self._conn.type_text, text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def press_key(self, key: str) -> Dict[str, Any]:
        try:
            await asyncio.to_thread(self._conn.key_press, key)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def key_down(self, key: str) -> Dict[str, Any]:
        try:
            await asyncio.to_thread(self._conn.key_down, key)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def key_up(self, key: str) -> Dict[str, Any]:
        try:
            await asyncio.to_thread(self._conn.key_up, key)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        try:
            await asyncio.to_thread(self._conn.hotkey, keys)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # -- Scrolling ----------------------------------------------------------

    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        try:
            await asyncio.to_thread(self._conn.scroll, x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        return await self.scroll(0, clicks)

    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        return await self.scroll(0, -clicks)

    # -- Screen info --------------------------------------------------------

    async def get_screen_size(self) -> Dict[str, Any]:
        try:
            w, h = await asyncio.to_thread(lambda: self._conn.screen_size)
            return {"success": True, "size": {"width": w, "height": h}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_cursor_position(self) -> Dict[str, Any]:
        try:
            x, y = self._conn.cursor_position
            return {"success": True, "position": {"x": x, "y": y}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Clipboard and run_command inherited from BaseAutomationHandler


# ---------------------------------------------------------------------------
# Accessibility Handler (stub)
# ---------------------------------------------------------------------------


class VNCAccessibilityHandler(BaseAccessibilityHandler):
    """Stub accessibility handler for VNC — no accessibility tree available."""

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        return {
            "success": True,
            "tree": {
                "role": "Window",
                "title": "VNC Remote Desktop",
                "position": {"x": 0, "y": 0},
                "size": {"width": 0, "height": 0},
                "children": [],
            },
        }

    async def find_element(
        self,
        role: Optional[str] = None,
        title: Optional[str] = None,
        value: Optional[str] = None,
    ) -> Dict[str, Any]:
        return {
            "success": False,
            "error": "Accessibility tree not available over VNC",
        }
