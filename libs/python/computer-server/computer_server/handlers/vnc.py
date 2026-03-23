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
    """Manages vncdotool client connections to a VNC server.

    Uses fresh connections per operation to avoid a bug in vncdotool's
    ThreadedVNCClientProxy where the Twisted deferred chain corrupts
    coordinate state after operations like refreshScreen.  Each public
    method creates its own connection, executes, and disconnects.

    All methods are synchronous (blocking) — the handler calls them via
    asyncio.to_thread to avoid blocking the event loop.
    """

    def __init__(self, host: str, port: int, password: str = ""):
        self._host = host
        self._port = port
        self._password = password
        # Track cursor position locally
        self._cursor_x: int = 0
        self._cursor_y: int = 0

    def _with_client(self, fn):
        """Execute fn(client) with a fresh VNC connection via the global Twisted reactor.

        Starts the reactor in a background thread on first use, then reuses it.
        ``fn(client)`` receives a raw ``VNCDoToolClient`` whose methods return
        Twisted Deferreds.
        """
        import threading

        from twisted.internet import defer, reactor
        from vncdotool.client import VNCDoToolFactory

        result_holder: list = [None]
        error_holder: list = [None]
        done_event = threading.Event()

        def _work():
            factory = VNCDoToolFactory()
            factory.password = self._password or None

            @defer.inlineCallbacks
            def _do():
                client = None
                try:
                    reactor.connectTCP(self._host, self._port, factory)
                    client = yield factory.deferred
                    res = yield defer.maybeDeferred(fn, client)
                    result_holder[0] = res
                except Exception as e:
                    error_holder[0] = e
                finally:
                    if client is not None and hasattr(client, "transport") and client.transport:
                        client.transport.loseConnection()
                    done_event.set()

            _do()

        # Ensure the reactor is running in a background thread
        if not reactor.running:
            t = threading.Thread(
                target=reactor.run,
                kwargs={"installSignalHandlers": False},
                daemon=True,
            )
            t.start()

        reactor.callFromThread(_work)
        done_event.wait(timeout=30)
        if not done_event.is_set():
            raise TimeoutError("VNC operation timed out")
        if error_holder[0] is not None:
            raise error_holder[0]
        return result_holder[0]

    def disconnect(self):
        pass  # No persistent connection to close

    def _reset_on_error(self):
        pass  # No persistent connection to reset

    # -- Screenshot ---------------------------------------------------------

    def capture_screenshot(self) -> bytes:
        """Capture the screen and return PNG bytes."""
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            yield client.refreshScreen(incremental=False)
            screen = client.screen
            buf = BytesIO()
            screen.save(buf, format="PNG")
            defer.returnValue(buf.getvalue())
        return self._with_client(_do)

    # -- Mouse --------------------------------------------------------------

    def mouse_move(self, x: int, y: int):
        self._with_client(lambda c: c.mouseMove(x, y))
        self._cursor_x = x
        self._cursor_y = y

    def mouse_click(self, x: int, y: int, button: int = 1, clicks: int = 1):
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            yield client.mouseMove(x, y)
            for _ in range(clicks):
                yield client.mousePress(button)
        self._with_client(_do)
        self._cursor_x = x
        self._cursor_y = y

    def mouse_down(self, x: int, y: int, button: int = 1):
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            yield client.mouseMove(x, y)
            yield client.mouseDown(button)
        self._with_client(_do)
        self._cursor_x = x
        self._cursor_y = y

    def mouse_up(self, x: int, y: int, button: int = 1):
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            yield client.mouseMove(x, y)
            yield client.mouseUp(button)
        self._with_client(_do)
        self._cursor_x = x
        self._cursor_y = y

    def mouse_drag(self, x: int, y: int, step: int = 1):
        from twisted.internet import defer

        sx, sy = self._cursor_x, self._cursor_y

        @defer.inlineCallbacks
        def _do(client):
            # Move in increments from current position to target
            dx, dy = x - sx, y - sy
            steps = max(abs(dx), abs(dy)) // max(step, 1)
            steps = max(steps, 1)
            yield client.mouseDown(1)
            for i in range(1, steps + 1):
                ix = sx + dx * i // steps
                iy = sy + dy * i // steps
                yield client.mouseMove(ix, iy)
            yield client.mouseUp(1)
        self._with_client(_do)
        self._cursor_x = x
        self._cursor_y = y

    def drag_to(self, start_x: int, start_y: int, end_x: int, end_y: int,
                button: int = 1, step: int = 5):
        """Drag from start to end on a single connection."""
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            yield client.mouseMove(start_x, start_y)
            yield client.mouseDown(button)
            # Manual drag in increments instead of mouseDrag (avoids doPoll)
            dx, dy = end_x - start_x, end_y - start_y
            steps = max(abs(dx), abs(dy)) // max(step, 1)
            steps = max(steps, 1)
            for i in range(1, steps + 1):
                ix = start_x + dx * i // steps
                iy = start_y + dy * i // steps
                yield client.mouseMove(ix, iy)
            yield client.mouseUp(button)
        self._with_client(_do)
        self._cursor_x = end_x
        self._cursor_y = end_y

    def drag_path(self, path: List[Tuple[int, int]], button: int = 1):
        """Drag along a path of points on a single connection."""
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            yield client.mouseMove(path[0][0], path[0][1])
            yield client.mouseDown(button)
            for px, py in path[1:]:
                yield client.mouseMove(px, py)
            yield client.mouseUp(button)
        self._with_client(_do)
        self._cursor_x = path[-1][0]
        self._cursor_y = path[-1][1]

    def scroll(self, x: int, y: int):
        """Scroll. y>0 = up, y<0 = down (matches macOS native handler convention).

        Uses arrow key presses instead of mouse buttons 4/5 because Apple's
        _VZVNCServer (used by Lume) does not translate RFB mouse buttons 4-7
        into scroll wheel events.
        """
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            if y > 0:
                for _ in range(y):
                    yield client.keyPress("up")
            elif y < 0:
                for _ in range(abs(y)):
                    yield client.keyPress("down")
            if x > 0:
                for _ in range(x):
                    yield client.keyPress("right")
            elif x < 0:
                for _ in range(abs(x)):
                    yield client.keyPress("left")
        self._with_client(_do)

    # -- Keyboard -----------------------------------------------------------

    def key_press(self, key: str):
        self._with_client(lambda c: c.keyPress(_translate_key(key)))

    def key_down(self, key: str):
        self._with_client(lambda c: c.keyDown(_translate_key(key)))

    def key_up(self, key: str):
        self._with_client(lambda c: c.keyUp(_translate_key(key)))

    def type_text(self, text: str):
        """Type text character by character, handling shift for uppercase/symbols."""
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            for ch in text:
                if ch == " ":
                    yield client.keyPress("space")
                elif ch == "\n":
                    yield client.keyPress("enter")
                elif ch == "\t":
                    yield client.keyPress("tab")
                else:
                    yield client.keyPress(ch)
        self._with_client(_do)

    def hotkey(self, keys: List[str]):
        """Press a key combination (e.g. ['command', 'a'])."""
        translated = [_translate_key(k) for k in keys]
        combo = "-".join(translated)
        self._with_client(lambda c: c.keyPress(combo))

    # -- Clipboard ----------------------------------------------------------

    def paste_text(self, text: str):
        self._with_client(lambda c: c.paste(text))

    # -- Info ---------------------------------------------------------------

    @property
    def screen_size(self) -> Tuple[int, int]:
        from twisted.internet import defer

        @defer.inlineCallbacks
        def _do(client):
            yield client.refreshScreen(incremental=False)
            if client.screen is not None:
                defer.returnValue(client.screen.size)
            defer.returnValue((0, 0))
        return self._with_client(_do)

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

    async def middle_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        try:
            cx, cy = self._resolve_coords(x, y)
            await asyncio.to_thread(self._conn.mouse_click, cx, cy, 2)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
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
            step = max(1, int(1 / max(duration, 0.01) * 5))
            await asyncio.to_thread(self._conn.drag_to, cx, cy, x, y, btn, step)
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
            await asyncio.to_thread(self._conn.drag_path, path, btn)
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
        return await self.scroll(0, -clicks)

    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        return await self.scroll(0, clicks)

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
