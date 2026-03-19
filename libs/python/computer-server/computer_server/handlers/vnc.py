"""
VNC backend for automation and accessibility handlers.

Connects to a VNC (RFB) server to perform screenshots, mouse clicks, keyboard
input, and scrolling over the network. This bypasses macOS TCC permissions
entirely since all interaction happens through the VNC protocol.

Usage:
    Set env vars before starting the computer-server:
        CUA_VNC_HOST=127.0.0.1
        CUA_VNC_PORT=5900
        CUA_VNC_PASSWORD=secret

    Or the factory will auto-detect when CUA_VNC_HOST is set.
"""

import asyncio
import base64
import hashlib
import logging
import os
import struct
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

from PIL import Image

from .base import BaseAccessibilityHandler, BaseAutomationHandler

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# RFB protocol constants
# ---------------------------------------------------------------------------

# Client → Server message types
RFB_SET_PIXEL_FORMAT = 0
RFB_SET_ENCODINGS = 2
RFB_FRAMEBUFFER_UPDATE_REQUEST = 3
RFB_KEY_EVENT = 4
RFB_POINTER_EVENT = 5
RFB_CLIENT_CUT_TEXT = 6

# Server → Client message types
RFB_FRAMEBUFFER_UPDATE = 0

# Encoding types
RFB_ENCODING_RAW = 0
RFB_ENCODING_DESKTOP_SIZE = -223

# VNC auth type
RFB_AUTH_NONE = 1
RFB_AUTH_VNC = 2

# Scroll button masks (RFB protocol: buttons 4/5 are scroll wheel)
RFB_SCROLL_UP = 0x08  # button 4
RFB_SCROLL_DOWN = 0x10  # button 5


# ---------------------------------------------------------------------------
# X11 keysym mappings
# ---------------------------------------------------------------------------

_SPECIAL_KEYS = {
    "return": 0xFF0D,
    "enter": 0xFF0D,
    "tab": 0xFF09,
    "escape": 0xFF1B,
    "esc": 0xFF1B,
    "backspace": 0xFF08,
    "delete": 0xFFFF,
    "space": 0x0020,
    "up": 0xFF52,
    "down": 0xFF54,
    "left": 0xFF51,
    "right": 0xFF53,
    "home": 0xFF50,
    "end": 0xFF57,
    "pageup": 0xFF55,
    "page_up": 0xFF55,
    "pagedown": 0xFF56,
    "page_down": 0xFF56,
    "insert": 0xFF63,
    "f1": 0xFFBE,
    "f2": 0xFFBF,
    "f3": 0xFFC0,
    "f4": 0xFFC1,
    "f5": 0xFFC2,
    "f6": 0xFFC3,
    "f7": 0xFFC4,
    "f8": 0xFFC5,
    "f9": 0xFFC6,
    "f10": 0xFFC7,
    "f11": 0xFFC8,
    "f12": 0xFFC9,
    "shift": 0xFFE1,
    "shift_l": 0xFFE1,
    "shift_r": 0xFFE2,
    "ctrl": 0xFFE3,
    "ctrl_l": 0xFFE3,
    "ctrl_r": 0xFFE4,
    "control": 0xFFE3,
    "control_l": 0xFFE3,
    "control_r": 0xFFE4,
    # VNC/macOS quirk: option maps to Meta, command maps to Alt
    "alt": 0xFFE9,
    "alt_l": 0xFFE9,
    "alt_r": 0xFFEA,
    "option": 0xFFE7,
    "option_l": 0xFFE7,
    "option_r": 0xFFE8,
    "meta": 0xFFE7,
    "meta_l": 0xFFE7,
    "meta_r": 0xFFE8,
    "command": 0xFFE9,
    "cmd": 0xFFE9,
    "super": 0xFFEB,
    "super_l": 0xFFEB,
    "super_r": 0xFFEC,
    "caps_lock": 0xFFE5,
    "num_lock": 0xFF7F,
}

# Characters that require Shift
_SHIFT_CHARS = set('~!@#$%^&*()_+{}|:"<>?ABCDEFGHIJKLMNOPQRSTUVWXYZ')

# Shifted symbol → base keysym
_SHIFTED_SYMBOL_MAP = {
    "~": 0x0060,
    "!": 0x0031,
    "@": 0x0032,
    "#": 0x0033,
    "$": 0x0034,
    "%": 0x0035,
    "^": 0x0036,
    "&": 0x0037,
    "*": 0x0038,
    "(": 0x0039,
    ")": 0x0030,
    "_": 0x002D,
    "+": 0x003D,
    "{": 0x005B,
    "}": 0x005D,
    "|": 0x005C,
    ":": 0x003B,
    '"': 0x0027,
    "<": 0x002C,
    ">": 0x002E,
    "?": 0x002F,
}


def _key_to_keysym(key: str) -> int:
    """Convert a key name or character to an X11 keysym."""
    lower = key.lower()
    if lower in _SPECIAL_KEYS:
        return _SPECIAL_KEYS[lower]
    if len(key) == 1:
        # Single character — use its Unicode code point (works for ASCII)
        return ord(key)
    raise ValueError(f"Unknown key: {key}")


def _char_to_keysym(ch: str) -> Tuple[int, bool]:
    """Convert a character to (keysym, needs_shift)."""
    if ch in _SHIFTED_SYMBOL_MAP:
        return _SHIFTED_SYMBOL_MAP[ch], True
    if ch.isupper():
        return ord(ch.lower()), True
    return ord(ch), False


# ---------------------------------------------------------------------------
# VNC DES auth helper
# ---------------------------------------------------------------------------


def _vnc_des_encrypt(password: str, challenge: bytes) -> bytes:
    """Encrypt a VNC auth challenge using DES-ECB with bit-reversed key."""
    try:
        from Crypto.Cipher import DES
    except ImportError:
        try:
            from Cryptodome.Cipher import DES
        except ImportError:
            raise ImportError(
                "VNC authentication requires pycryptodome. "
                "Install with: pip install pycryptodome"
            )

    # Pad/truncate password to 8 bytes
    key_bytes = (password.encode("ascii") + b"\x00" * 8)[:8]

    # VNC quirk: reverse bits of each byte
    def reverse_bits(b: int) -> int:
        result = 0
        for _ in range(8):
            result = (result << 1) | (b & 1)
            b >>= 1
        return result

    key_bytes = bytes(reverse_bits(b) for b in key_bytes)

    cipher = DES.new(key_bytes, DES.MODE_ECB)
    return cipher.encrypt(challenge[:8]) + cipher.encrypt(challenge[8:16])


# ---------------------------------------------------------------------------
# RFB Client
# ---------------------------------------------------------------------------


class RFBClient:
    """Async RFB (VNC) protocol client.

    Handles connection, authentication, framebuffer capture, and input events.
    """

    def __init__(self, host: str, port: int, password: str = ""):
        self.host = host
        self.port = port
        self.password = password
        self.reader: Optional[asyncio.StreamReader] = None
        self.writer: Optional[asyncio.StreamWriter] = None
        self.width: int = 0
        self.height: int = 0
        self.bpp: int = 32
        self.depth: int = 24
        self.big_endian: bool = False
        self.true_colour: bool = True
        self.red_max: int = 255
        self.green_max: int = 255
        self.blue_max: int = 255
        self.red_shift: int = 16
        self.green_shift: int = 8
        self.blue_shift: int = 0
        self._name: str = ""
        self._connected: bool = False
        self._lock = asyncio.Lock()
        # Track cursor position for relative operations
        self._cursor_x: int = 0
        self._cursor_y: int = 0

    @property
    def connected(self) -> bool:
        return self._connected

    async def connect(self) -> None:
        """Establish TCP connection and perform RFB handshake."""
        self.reader, self.writer = await asyncio.wait_for(
            asyncio.open_connection(self.host, self.port), timeout=10.0
        )
        await self._handshake()
        self._connected = True
        logger.info(f"VNC connected to {self.host}:{self.port} ({self.width}x{self.height})")

    async def disconnect(self) -> None:
        """Close the connection."""
        self._connected = False
        if self.writer:
            self.writer.close()
            try:
                await self.writer.wait_closed()
            except Exception:
                pass
            self.writer = None
            self.reader = None

    async def _read_exact(self, n: int) -> bytes:
        """Read exactly n bytes."""
        assert self.reader is not None
        data = await asyncio.wait_for(self.reader.readexactly(n), timeout=30.0)
        return data

    async def _send(self, data: bytes) -> None:
        """Send data to server."""
        assert self.writer is not None
        self.writer.write(data)
        await self.writer.drain()

    async def _handshake(self) -> None:
        """Perform RFB protocol handshake."""
        # 1. Protocol version
        server_version = await self._read_exact(12)
        logger.debug(f"Server version: {server_version!r}")
        await self._send(b"RFB 003.008\n")

        # 2. Security types
        num_types = struct.unpack("!B", await self._read_exact(1))[0]
        if num_types == 0:
            # Connection failed — read reason
            reason_len = struct.unpack("!I", await self._read_exact(4))[0]
            reason = (await self._read_exact(reason_len)).decode("utf-8", errors="replace")
            raise ConnectionError(f"VNC connection refused: {reason}")

        security_types = list(await self._read_exact(num_types))
        logger.debug(f"Security types: {security_types}")

        # Choose auth type
        if RFB_AUTH_NONE in security_types and not self.password:
            await self._send(struct.pack("!B", RFB_AUTH_NONE))
        elif RFB_AUTH_VNC in security_types:
            await self._send(struct.pack("!B", RFB_AUTH_VNC))
            # VNC auth challenge
            challenge = await self._read_exact(16)
            response = _vnc_des_encrypt(self.password, challenge)
            await self._send(response)
        else:
            raise ConnectionError(f"No supported security type (got {security_types})")

        # 3. Security result
        result = struct.unpack("!I", await self._read_exact(4))[0]
        if result != 0:
            # Try to read failure reason
            try:
                reason_len = struct.unpack("!I", await self._read_exact(4))[0]
                reason = (await self._read_exact(reason_len)).decode("utf-8", errors="replace")
            except Exception:
                reason = "unknown"
            raise ConnectionError(f"VNC authentication failed: {reason}")

        # 4. ClientInit — shared flag = 1 (allow other clients)
        await self._send(struct.pack("!B", 1))

        # 5. ServerInit
        header = await self._read_exact(24)
        self.width, self.height = struct.unpack("!HH", header[0:4])
        # Pixel format (16 bytes at offset 4)
        pf = header[4:20]
        self.bpp = pf[0]
        self.depth = pf[1]
        self.big_endian = bool(pf[2])
        self.true_colour = bool(pf[3])
        self.red_max, self.green_max, self.blue_max = struct.unpack("!HHH", pf[4:10])
        self.red_shift, self.green_shift, self.blue_shift = pf[10], pf[11], pf[12]

        # Desktop name
        name_len = struct.unpack("!I", header[20:24])[0]
        self._name = (await self._read_exact(name_len)).decode("utf-8", errors="replace")

        # 6. Set encodings (Raw + DesktopSize)
        encodings = [RFB_ENCODING_RAW, RFB_ENCODING_DESKTOP_SIZE]
        msg = struct.pack("!BxH", RFB_SET_ENCODINGS, len(encodings))
        for enc in encodings:
            msg += struct.pack("!i", enc)
        await self._send(msg)

    async def capture_framebuffer(self) -> Image.Image:
        """Request and read a full framebuffer update, returning a PIL Image."""
        async with self._lock:
            # Request full framebuffer update (incremental=0)
            msg = struct.pack(
                "!BBHHHH",
                RFB_FRAMEBUFFER_UPDATE_REQUEST,
                0,  # non-incremental
                0, 0,  # x, y
                self.width, self.height,
            )
            await self._send(msg)

            # Read response
            return await self._read_framebuffer_update()

    async def _read_framebuffer_update(self) -> Image.Image:
        """Read a FramebufferUpdate message and assemble into an image."""
        # Allocate pixel buffer
        pixels = bytearray(self.width * self.height * 4)  # RGBA

        while True:
            msg_type = struct.unpack("!B", await self._read_exact(1))[0]

            if msg_type != RFB_FRAMEBUFFER_UPDATE:
                # Skip unexpected messages
                # Bell (2): no payload
                # ServerCutText (3): 7 bytes header + text
                if msg_type == 2:
                    continue
                elif msg_type == 3:
                    _ = await self._read_exact(3)  # padding
                    text_len = struct.unpack("!I", await self._read_exact(4))[0]
                    _ = await self._read_exact(text_len)
                    continue
                else:
                    logger.warning(f"Unexpected RFB message type: {msg_type}")
                    continue

            # FramebufferUpdate header
            _ = await self._read_exact(1)  # padding
            num_rects = struct.unpack("!H", await self._read_exact(2))[0]

            for _ in range(num_rects):
                rect_header = await self._read_exact(12)
                rx, ry, rw, rh = struct.unpack("!HH HH", rect_header[0:8])
                encoding = struct.unpack("!i", rect_header[8:12])[0]

                if encoding == RFB_ENCODING_RAW:
                    bytes_per_pixel = self.bpp // 8
                    data = await self._read_exact(rw * rh * bytes_per_pixel)
                    self._copy_rect_to_buffer(pixels, rx, ry, rw, rh, data)

                elif encoding == RFB_ENCODING_DESKTOP_SIZE:
                    self.width = rw
                    self.height = rh
                    pixels = bytearray(self.width * self.height * 4)

                else:
                    logger.warning(f"Unsupported encoding: {encoding}")

            # Build PIL image from pixel buffer
            img = Image.frombytes("RGBA", (self.width, self.height), bytes(pixels))
            return img

    def _copy_rect_to_buffer(
        self,
        buffer: bytearray,
        x: int,
        y: int,
        w: int,
        h: int,
        data: bytes,
    ) -> None:
        """Copy rectangle pixel data into the framebuffer."""
        bytes_per_pixel = self.bpp // 8
        stride = self.width * 4  # output stride (RGBA)

        for row in range(h):
            src_offset = row * w * bytes_per_pixel
            dst_offset = ((y + row) * self.width + x) * 4

            for col in range(w):
                sp = src_offset + col * bytes_per_pixel
                dp = dst_offset + col * 4

                if bytes_per_pixel == 4:
                    pixel = struct.unpack_from("I" if not self.big_endian else "!I", data, sp)[0]
                elif bytes_per_pixel == 2:
                    pixel = struct.unpack_from("H" if not self.big_endian else "!H", data, sp)[0]
                else:
                    pixel = data[sp]

                r = ((pixel >> self.red_shift) & self.red_max) * 255 // max(self.red_max, 1)
                g = ((pixel >> self.green_shift) & self.green_max) * 255 // max(self.green_max, 1)
                b = ((pixel >> self.blue_shift) & self.blue_max) * 255 // max(self.blue_max, 1)

                buffer[dp] = r
                buffer[dp + 1] = g
                buffer[dp + 2] = b
                buffer[dp + 3] = 255

    # -----------------------------------------------------------------------
    # Input events
    # -----------------------------------------------------------------------

    async def pointer_event(self, x: int, y: int, button_mask: int = 0) -> None:
        """Send a pointer (mouse) event."""
        x = max(0, min(x, self.width - 1))
        y = max(0, min(y, self.height - 1))
        self._cursor_x = x
        self._cursor_y = y
        await self._send(struct.pack("!BBHH", RFB_POINTER_EVENT, button_mask, x, y))

    async def key_event(self, keysym: int, down: bool) -> None:
        """Send a key press/release event."""
        await self._send(struct.pack("!BBxxI", RFB_KEY_EVENT, 1 if down else 0, keysym))

    async def send_key(self, keysym: int) -> None:
        """Press and release a key."""
        await self.key_event(keysym, True)
        await asyncio.sleep(0.02)
        await self.key_event(keysym, False)

    async def send_text(self, text: str, delay: float = 0.03) -> None:
        """Type a string character by character."""
        for ch in text:
            keysym, needs_shift = _char_to_keysym(ch)
            if needs_shift:
                await self.key_event(_SPECIAL_KEYS["shift"], True)
            await self.key_event(keysym, True)
            await asyncio.sleep(0.01)
            await self.key_event(keysym, False)
            if needs_shift:
                await self.key_event(_SPECIAL_KEYS["shift"], False)
            await asyncio.sleep(delay)

    async def mouse_click(
        self, x: int, y: int, button_mask: int = 1, clicks: int = 1
    ) -> None:
        """Click at coordinates. button_mask: 1=left, 2=middle, 4=right."""
        # Move to position
        await self.pointer_event(x, y, 0)
        await asyncio.sleep(0.03)
        for _ in range(clicks):
            # Press
            await self.pointer_event(x, y, button_mask)
            await asyncio.sleep(0.05)
            # Release
            await self.pointer_event(x, y, 0)
            await asyncio.sleep(0.05)

    async def client_cut_text(self, text: str) -> None:
        """Send clipboard text to the VNC server."""
        encoded = text.encode("latin-1", errors="replace")
        msg = struct.pack("!BxxxI", RFB_CLIENT_CUT_TEXT, len(encoded))
        await self._send(msg + encoded)


# ---------------------------------------------------------------------------
# Automation Handler
# ---------------------------------------------------------------------------


class VNCAutomationHandler(BaseAutomationHandler):
    """Automation handler that operates via VNC/RFB protocol.

    All screenshot, mouse, and keyboard operations go through a VNC
    connection, bypassing local TCC permissions entirely.
    """

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 5900,
        password: str = "",
    ):
        self._host = host
        self._port = port
        self._password = password
        self._client: Optional[RFBClient] = None

    async def _ensure_connected(self) -> RFBClient:
        """Lazily connect (or reconnect) to the VNC server."""
        if self._client is not None and self._client.connected:
            return self._client
        client = RFBClient(self._host, self._port, self._password)
        await client.connect()
        self._client = client
        return client

    # -- Screenshot ---------------------------------------------------------

    async def screenshot(self) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            img = await client.capture_framebuffer()
            buf = BytesIO()
            img.save(buf, format="PNG", optimize=True)
            image_data = base64.b64encode(buf.getvalue()).decode()
            return {"success": True, "image_data": image_data}
        except Exception as e:
            logger.error(f"VNC screenshot error: {e}")
            # Reset connection on failure
            if self._client:
                await self._client.disconnect()
                self._client = None
            return {"success": False, "error": f"VNC screenshot error: {e}"}

    # -- Mouse actions ------------------------------------------------------

    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            cx, cy = self._resolve_coords(client, x, y)
            await client.mouse_click(cx, cy, button_mask=1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            cx, cy = self._resolve_coords(client, x, y)
            await client.mouse_click(cx, cy, button_mask=4)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def middle_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            cx, cy = self._resolve_coords(client, x, y)
            await client.mouse_click(cx, cy, button_mask=2)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def double_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            cx, cy = self._resolve_coords(client, x, y)
            await client.mouse_click(cx, cy, button_mask=1, clicks=2)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            await client.pointer_event(x, y, 0)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            cx, cy = self._resolve_coords(client, x, y)
            mask = {"left": 1, "middle": 2, "right": 4}.get(button, 1)
            await client.pointer_event(cx, cy, mask)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            cx, cy = self._resolve_coords(client, x, y)
            await client.pointer_event(cx, cy, 0)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            mask = {"left": 1, "middle": 2, "right": 4}.get(button, 1)
            sx, sy = client._cursor_x, client._cursor_y
            steps = max(int(duration * 60), 10)
            # Press
            await client.pointer_event(sx, sy, mask)
            # Interpolate
            for i in range(1, steps + 1):
                t = i / steps
                ix = int(sx + (x - sx) * t)
                iy = int(sy + (y - sy) * t)
                await client.pointer_event(ix, iy, mask)
                await asyncio.sleep(duration / steps)
            # Release
            await client.pointer_event(x, y, 0)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            mask = {"left": 1, "middle": 2, "right": 4}.get(button, 1)
            if not path:
                return {"success": False, "error": "Empty path"}
            # Press at first point
            await client.pointer_event(path[0][0], path[0][1], mask)
            step_delay = duration / max(len(path), 1)
            for px, py in path[1:]:
                await client.pointer_event(px, py, mask)
                await asyncio.sleep(step_delay)
            # Release
            await client.pointer_event(path[-1][0], path[-1][1], 0)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # -- Keyboard actions ---------------------------------------------------

    async def type_text(self, text: str) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            await client.send_text(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def press_key(self, key: str) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            keysym = _key_to_keysym(key)
            await client.send_key(keysym)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def key_down(self, key: str) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            keysym = _key_to_keysym(key)
            await client.key_event(keysym, True)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def key_up(self, key: str) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            keysym = _key_to_keysym(key)
            await client.key_event(keysym, False)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            keysyms = [_key_to_keysym(k) for k in keys]
            # Press all modifiers, then last key
            for ks in keysyms:
                await client.key_event(ks, True)
                await asyncio.sleep(0.02)
            await asyncio.sleep(0.1)
            # Release in reverse order
            for ks in reversed(keysyms):
                await client.key_event(ks, False)
                await asyncio.sleep(0.02)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # -- Scrolling ----------------------------------------------------------

    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            cx, cy = client._cursor_x, client._cursor_y
            # Vertical scroll
            if y != 0:
                btn = RFB_SCROLL_UP if y < 0 else RFB_SCROLL_DOWN
                for _ in range(abs(y)):
                    await client.pointer_event(cx, cy, btn)
                    await asyncio.sleep(0.02)
                    await client.pointer_event(cx, cy, 0)
                    await asyncio.sleep(0.02)
            # Horizontal scroll (buttons 6/7 — not universally supported)
            if x != 0:
                btn = 0x20 if x > 0 else 0x40
                for _ in range(abs(x)):
                    await client.pointer_event(cx, cy, btn)
                    await asyncio.sleep(0.02)
                    await client.pointer_event(cx, cy, 0)
                    await asyncio.sleep(0.02)
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
            client = await self._ensure_connected()
            return {
                "success": True,
                "size": {"width": client.width, "height": client.height},
            }
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_cursor_position(self) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            return {
                "success": True,
                "position": {"x": client._cursor_x, "y": client._cursor_y},
            }
        except Exception as e:
            return {"success": False, "error": str(e)}

    # -- Clipboard ----------------------------------------------------------

    async def copy_to_clipboard(self) -> Dict[str, Any]:
        # VNC protocol doesn't support reading remote clipboard
        return {"success": False, "error": "Reading clipboard not supported over VNC"}

    async def set_clipboard(self, text: str) -> Dict[str, Any]:
        try:
            client = await self._ensure_connected()
            await client.client_cut_text(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # -- Shell command ------------------------------------------------------

    async def run_command(self, command: str) -> Dict[str, Any]:
        # Cannot run commands via VNC — this would need SSH
        return {
            "success": False,
            "error": "Shell commands not supported over VNC. Use SSH instead.",
        }

    # -- Helpers ------------------------------------------------------------

    @staticmethod
    def _resolve_coords(
        client: RFBClient, x: Optional[int], y: Optional[int]
    ) -> Tuple[int, int]:
        """Resolve coordinates, falling back to current cursor position."""
        if x is not None and y is not None:
            return x, y
        return client._cursor_x, client._cursor_y


# ---------------------------------------------------------------------------
# Accessibility Handler (stub for VNC)
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
