"""QMPTransport — control a QEMU VM directly via QMP (no guest agent required).

Uses the QEMU Machine Protocol to provide mouse, keyboard, and screenshot
control without needing computer-server installed inside the guest OS.
This enables sandboxing of stock OS images (e.g. Android-x86, raw Linux ISOs).

QMP commands used:
  - screendump          → screenshot (PNG via PPM conversion)
  - input-send-event    → mouse move, click, scroll
  - send-key            → keyboard input
  - query-status        → VM status checks

For shell access, this transport uses QEMU Guest Agent (QGA) over virtio-serial
when available, or raises NotImplementedError for raw VMs without an agent.
"""

from __future__ import annotations

import asyncio
import json
import logging
import struct
import tempfile
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from cua_sandbox.transport.base import Transport

logger = logging.getLogger(__name__)

# ── QMP key name mapping ─────────────────────────────────────────────────────
# Maps common key names to QEMU QCode values.
# Full list: https://github.com/qemu/qemu/blob/master/ui/input-keymap-qcode-to-qnum.c

_KEY_MAP: Dict[str, str] = {
    # Letters
    **{c: c for c in "abcdefghijklmnopqrstuvwxyz"},
    # Numbers
    **{str(i): str(i) for i in range(10)},
    # Special keys
    "enter": "ret",
    "return": "ret",
    "ret": "ret",
    "escape": "esc",
    "esc": "esc",
    "tab": "tab",
    "space": "spc",
    " ": "spc",
    "backspace": "backspace",
    "delete": "delete",
    "del": "delete",
    "insert": "insert",
    "ins": "insert",
    "home": "home",
    "end": "end",
    "pageup": "pgup",
    "page_up": "pgup",
    "pgup": "pgup",
    "pagedown": "pgdn",
    "page_down": "pgdn",
    "pgdn": "pgdn",
    "up": "up",
    "down": "down",
    "left": "left",
    "right": "right",
    # Modifiers
    "shift": "shift",
    "ctrl": "ctrl",
    "control": "ctrl",
    "alt": "alt",
    "meta": "meta_l",
    "super": "meta_l",
    "command": "meta_l",
    "cmd": "meta_l",
    "win": "meta_l",
    # Function keys
    **{f"f{i}": f"f{i}" for i in range(1, 13)},
    # Punctuation
    "-": "minus",
    "=": "equal",
    "[": "bracket_left",
    "]": "bracket_right",
    "\\": "backslash",
    ";": "semicolon",
    "'": "apostrophe",
    ",": "comma",
    ".": "dot",
    "/": "slash",
    "`": "grave_accent",
}

# Mouse button mapping
_BTN_MAP: Dict[str, int] = {"left": 0, "middle": 1, "right": 2}


class QMPTransport(Transport):
    """Transport that controls a QEMU VM via QMP socket.

    Provides screenshot, mouse, and keyboard control without any guest agent.
    Shell commands require QEMU Guest Agent (virtio-serial) in the guest.
    """

    def __init__(
        self,
        qmp_host: str = "127.0.0.1",
        qmp_port: int = 4444,
        *,
        guest_agent_port: Optional[int] = None,
        screen_width: int = 1280,
        screen_height: int = 720,
        environment: str = "linux",
    ):
        self._host = qmp_host
        self._port = qmp_port
        self._ga_port = guest_agent_port
        self._screen_w = screen_width
        self._screen_h = screen_height
        self._environment = environment
        self._reader: Optional[asyncio.StreamReader] = None
        self._writer: Optional[asyncio.StreamWriter] = None
        self._tmpdir = Path(tempfile.mkdtemp(prefix="cua-qmp-"))

    # ── Connection lifecycle ──────────────────────────────────────────────

    async def connect(self) -> None:
        self._reader, self._writer = await asyncio.open_connection(self._host, self._port)
        # QMP greeting — read and discard
        await self._read_response()
        # Negotiate capabilities
        await self._execute("qmp_capabilities")
        logger.info(f"QMP connected to {self._host}:{self._port}")

    async def disconnect(self) -> None:
        if self._writer:
            self._writer.close()
            try:
                await self._writer.wait_closed()
            except Exception:
                pass
            self._writer = None
            self._reader = None
        # Clean up temp files
        import shutil

        shutil.rmtree(self._tmpdir, ignore_errors=True)

    # ── Low-level QMP protocol ────────────────────────────────────────────

    async def _execute(self, command: str, arguments: Optional[Dict] = None) -> Dict:
        """Send a QMP command and return the response."""
        assert self._writer and self._reader, "QMP not connected"
        msg: Dict[str, Any] = {"execute": command}
        if arguments:
            msg["arguments"] = arguments
        payload = json.dumps(msg).encode() + b"\n"
        self._writer.write(payload)
        await self._writer.drain()
        return await self._read_response()

    async def _read_response(self) -> Dict:
        """Read a single QMP JSON response, skipping async events."""
        assert self._reader
        while True:
            line = await self._reader.readline()
            if not line:
                raise ConnectionError("QMP connection closed")
            data = json.loads(line)
            # Skip async events ({"event": ...}), return commands/greetings
            if "event" not in data:
                if "error" in data:
                    raise RuntimeError(f"QMP error: {data['error']}")
                return data

    # ── Transport interface ───────────────────────────────────────────────

    async def send(self, action: str, **params: Any) -> Any:
        """Dispatch interface actions to QMP commands."""
        handler = _ACTION_DISPATCH.get(action)
        if handler:
            return await handler(self, **params)
        raise NotImplementedError(
            f"QMP transport does not support action {action!r}. "
            f"Supported: {', '.join(sorted(_ACTION_DISPATCH.keys()))}"
        )

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        """Capture a screenshot via QMP screendump → PPM → PNG."""
        ppm_path = str(self._tmpdir / "screen.ppm")
        await self._execute("screendump", {"filename": ppm_path})
        # Give QEMU a moment to write the file
        await asyncio.sleep(0.1)
        ppm_file = Path(ppm_path)
        if not ppm_file.exists():
            raise RuntimeError(f"screendump failed — {ppm_path} not created")
        png_bytes = _ppm_to_png(ppm_file.read_bytes())
        # Update screen dimensions from the PPM
        w, h = _ppm_dimensions(ppm_file.read_bytes())
        if w and h:
            self._screen_w = w
            self._screen_h = h
        from cua_sandbox.transport.base import convert_screenshot

        return convert_screenshot(png_bytes, format, quality)

    async def get_screen_size(self) -> Dict[str, int]:
        return {"width": self._screen_w, "height": self._screen_h}

    async def get_environment(self) -> str:
        return self._environment

    # ── Mouse actions ─────────────────────────────────────────────────────

    async def _mouse_move(self, x: int, y: int, **_: Any) -> None:
        """Move the mouse to absolute coordinates via input-send-event."""
        await self._execute(
            "input-send-event",
            {
                "events": [
                    {"type": "abs", "data": {"axis": "x", "value": _abs_coord(x, self._screen_w)}},
                    {"type": "abs", "data": {"axis": "y", "value": _abs_coord(y, self._screen_h)}},
                ]
            },
        )

    async def _mouse_click(self, x: int, y: int, button: str = "left", **_: Any) -> None:
        """Move to (x,y) then press+release a mouse button."""
        btn = _BTN_MAP.get(button, 0)
        await self._execute(
            "input-send-event",
            {
                "events": [
                    {"type": "abs", "data": {"axis": "x", "value": _abs_coord(x, self._screen_w)}},
                    {"type": "abs", "data": {"axis": "y", "value": _abs_coord(y, self._screen_h)}},
                    {"type": "btn", "data": {"down": True, "button": _qmp_btn(btn)}},
                    {"type": "btn", "data": {"down": False, "button": _qmp_btn(btn)}},
                ]
            },
        )

    async def _mouse_double_click(self, x: int, y: int, **_: Any) -> None:
        await self._mouse_click(x, y, "left")
        await asyncio.sleep(0.05)
        await self._mouse_click(x, y, "left")

    async def _mouse_right_click(self, x: int, y: int, **_: Any) -> None:
        await self._mouse_click(x, y, "right")

    async def _mouse_down(self, x: int, y: int, button: str = "left", **_: Any) -> None:
        btn = _BTN_MAP.get(button, 0)
        await self._execute(
            "input-send-event",
            {
                "events": [
                    {"type": "abs", "data": {"axis": "x", "value": _abs_coord(x, self._screen_w)}},
                    {"type": "abs", "data": {"axis": "y", "value": _abs_coord(y, self._screen_h)}},
                    {"type": "btn", "data": {"down": True, "button": _qmp_btn(btn)}},
                ]
            },
        )

    async def _mouse_up(self, x: int, y: int, button: str = "left", **_: Any) -> None:
        btn = _BTN_MAP.get(button, 0)
        await self._execute(
            "input-send-event",
            {
                "events": [
                    {"type": "abs", "data": {"axis": "x", "value": _abs_coord(x, self._screen_w)}},
                    {"type": "abs", "data": {"axis": "y", "value": _abs_coord(y, self._screen_h)}},
                    {"type": "btn", "data": {"down": False, "button": _qmp_btn(btn)}},
                ]
            },
        )

    async def _mouse_scroll(
        self, x: int, y: int, scroll_x: int = 0, scroll_y: int = 3, **_: Any
    ) -> None:
        """Scroll via button 4/5 (up/down) presses."""
        # Move to position first
        await self._mouse_move(x, y)
        # QMP wheel: positive scroll_y = scroll up (button 4), negative = down (button 5)
        if scroll_y > 0:
            btn_name = "wheel-up"
        elif scroll_y < 0:
            btn_name = "wheel-down"
        else:
            return
        for _ in range(abs(scroll_y)):
            await self._execute(
                "input-send-event",
                {
                    "events": [
                        {"type": "btn", "data": {"down": True, "button": btn_name}},
                        {"type": "btn", "data": {"down": False, "button": btn_name}},
                    ]
                },
            )

    async def _drag(
        self, start_x: int, start_y: int, end_x: int, end_y: int, button: str = "left", **_: Any
    ) -> None:
        await self._mouse_down(start_x, start_y, button)
        await asyncio.sleep(0.05)
        # Interpolate a few intermediate points for smoother drag
        steps = 10
        for i in range(1, steps + 1):
            ix = start_x + (end_x - start_x) * i // steps
            iy = start_y + (end_y - start_y) * i // steps
            await self._mouse_move(ix, iy)
            await asyncio.sleep(0.01)
        await self._mouse_up(end_x, end_y, button)

    # ── Keyboard actions ──────────────────────────────────────────────────

    async def _type_text(self, text: str, **_: Any) -> None:
        """Type text character by character via send-key."""
        for ch in text:
            qcode = _KEY_MAP.get(ch.lower())
            if qcode is None:
                logger.warning(f"QMP: unmapped character {ch!r}, skipping")
                continue
            keys: List[Dict] = []
            if ch.isupper() or ch in '~!@#$%^&*()_+{}|:"<>?':
                keys.append({"type": "qcode", "data": "shift"})
            keys.append({"type": "qcode", "data": qcode})
            await self._execute("send-key", {"keys": keys, "hold-time": 50})
            await asyncio.sleep(0.02)

    async def _hotkey(self, keys: List[str], **_: Any) -> None:
        """Press a key combination via send-key."""
        qcodes = []
        for k in keys:
            qcode = _KEY_MAP.get(k.lower())
            if qcode is None:
                raise ValueError(f"Unknown key {k!r}")
            qcodes.append({"type": "qcode", "data": qcode})
        await self._execute("send-key", {"keys": qcodes, "hold-time": 100})

    async def _key_down(self, key: str, **_: Any) -> None:
        qcode = _KEY_MAP.get(key.lower())
        if qcode is None:
            raise ValueError(f"Unknown key {key!r}")
        await self._execute(
            "input-send-event",
            {
                "events": [
                    {
                        "type": "key",
                        "data": {"down": True, "key": {"type": "qcode", "data": qcode}},
                    },
                ]
            },
        )

    async def _key_up(self, key: str, **_: Any) -> None:
        qcode = _KEY_MAP.get(key.lower())
        if qcode is None:
            raise ValueError(f"Unknown key {key!r}")
        await self._execute(
            "input-send-event",
            {
                "events": [
                    {
                        "type": "key",
                        "data": {"down": False, "key": {"type": "qcode", "data": qcode}},
                    },
                ]
            },
        )

    # ── Shell (via QEMU Guest Agent if available) ─────────────────────────

    async def _run_command(self, command: str, timeout: int = 30, **_: Any) -> Dict:
        """Execute a shell command via QEMU Guest Agent (QGA).

        Requires virtio-serial guest agent running inside the VM.
        Falls back to a helpful error if QGA is not available.
        """
        raise NotImplementedError(
            "Shell commands require QEMU Guest Agent (virtio-serial) inside the guest. "
            "QMPTransport currently supports screenshot, mouse, and keyboard only. "
            "For shell access, use an image with computer-server installed."
        )


# ── Action dispatch table ─────────────────────────────────────────────────

_ACTION_DISPATCH: Dict[str, Any] = {
    # Mouse
    "left_click": QMPTransport._mouse_click,
    "right_click": QMPTransport._mouse_right_click,
    "double_click": QMPTransport._mouse_double_click,
    "move_cursor": QMPTransport._mouse_move,
    "scroll": QMPTransport._mouse_scroll,
    "mouse_down": QMPTransport._mouse_down,
    "mouse_up": QMPTransport._mouse_up,
    "drag": QMPTransport._drag,
    # Keyboard
    "type_text": QMPTransport._type_text,
    "hotkey": QMPTransport._hotkey,
    "key_down": QMPTransport._key_down,
    "key_up": QMPTransport._key_up,
    # Shell
    "run_command": QMPTransport._run_command,
}


# ── PPM → PNG conversion (pure-Python, no Pillow dependency) ──────────────


def _ppm_dimensions(data: bytes) -> Tuple[int, int]:
    """Parse width and height from a PPM (P6) header."""
    try:
        # P6\n<width> <height>\n<maxval>\n<pixels>
        header_end = data.index(b"\n", data.index(b"\n") + 1)
        parts = data[:header_end].split()
        return int(parts[1]), int(parts[2])
    except (ValueError, IndexError):
        return 0, 0


def _ppm_to_png(ppm_data: bytes) -> bytes:
    """Convert a PPM P6 file to PNG using zlib (no external deps)."""
    import zlib

    # Parse PPM header
    idx = 0
    # Magic
    assert ppm_data[:2] == b"P6", "Not a PPM P6 file"
    idx = ppm_data.index(b"\n", idx) + 1
    # Skip comments
    while ppm_data[idx : idx + 1] == b"#":
        idx = ppm_data.index(b"\n", idx) + 1
    # Width Height
    line_end = ppm_data.index(b"\n", idx)
    dims = ppm_data[idx:line_end].split()
    width, height = int(dims[0]), int(dims[1])
    idx = line_end + 1
    # Max value
    line_end = ppm_data.index(b"\n", idx)
    idx = line_end + 1
    # Pixel data (RGB, 3 bytes per pixel)
    pixels = ppm_data[idx:]

    # Build PNG
    def _png_chunk(chunk_type: bytes, data: bytes) -> bytes:
        c = chunk_type + data
        crc = struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)
        return struct.pack(">I", len(data)) + c + crc

    # IHDR
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)  # 8-bit RGB
    # IDAT — build raw image data with filter byte 0 per row
    raw = bytearray()
    stride = width * 3
    for y in range(height):
        raw.append(0)  # filter: None
        raw.extend(pixels[y * stride : (y + 1) * stride])
    compressed = zlib.compress(bytes(raw))

    png = b"\x89PNG\r\n\x1a\n"
    png += _png_chunk(b"IHDR", ihdr)
    png += _png_chunk(b"IDAT", compressed)
    png += _png_chunk(b"IEND", b"")
    return png


# ── Helpers ───────────────────────────────────────────────────────────────


def _abs_coord(pixel: int, screen_max: int) -> int:
    """Convert pixel coordinate to QMP absolute input value (0–32767)."""
    return max(0, min(32767, pixel * 32767 // max(screen_max, 1)))


def _qmp_btn(idx: int) -> str:
    """Map button index to QMP button name."""
    return {0: "left", 1: "middle", 2: "right"}.get(idx, "left")
