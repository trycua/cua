"""Cross-platform screen capture and info."""

import base64
import io
from typing import Tuple

from PIL import Image


def screenshot() -> Image.Image:
    """Capture a screenshot of all monitors and return a PIL Image.

    Tries PIL.ImageGrab first (works on Windows and macOS).
    Falls back to mss if ImageGrab fails (better Linux / multi-monitor support).
    """
    # Try PIL.ImageGrab (Windows + macOS)
    try:
        from PIL import ImageGrab

        img = ImageGrab.grab(all_screens=True)
        if isinstance(img, Image.Image):
            return img
    except Exception:
        pass

    # Fallback: mss (cross-platform, optional dependency)
    try:
        import mss  # type: ignore[import-untyped]
        import mss.tools  # type: ignore[import-untyped]

        with mss.mss() as sct:
            monitor = sct.monitors[0]  # combined virtual desktop
            sct_img = sct.grab(monitor)
            return Image.frombytes("RGB", sct_img.size, sct_img.bgra, "raw", "BGRX")
    except Exception as e:
        raise RuntimeError(f"screenshot failed: {e}") from e


def screenshot_bytes(format: str = "PNG") -> bytes:
    """Return screenshot as raw bytes in the specified image format."""
    img = screenshot()
    buf = io.BytesIO()
    img.save(buf, format=format)
    return buf.getvalue()


def screenshot_b64(format: str = "PNG") -> str:
    """Return screenshot as a base64-encoded string."""
    return base64.b64encode(screenshot_bytes(format)).decode()


def screen_size() -> Tuple[int, int]:
    """Return the total virtual desktop size as (width, height)."""
    img = screenshot()
    return img.size


def cursor_position() -> Tuple[int, int]:
    """Return the current cursor position as (x, y).

    Tries win32gui first on Windows (more reliable with DPI scaling).
    Falls back to pynput if win32 is unavailable (cross-platform fallback).
    """
    try:
        import win32gui  # type: ignore[import-untyped]

        pos = win32gui.GetCursorPos()
        return int(pos[0]), int(pos[1])
    except Exception:
        pass

    from pynput.mouse import Controller as _MouseController

    c = _MouseController()
    x, y = c.position
    return int(x), int(y)
