"""Cross-platform screen capture and info."""

import base64
import io
import os
import platform
from typing import Tuple

from PIL import Image


def _get_display_scale() -> float:
    """Detect the OS display scale factor (DPI scaling).

    Returns the ratio of physical pixels to logical pixels:
    - macOS Retina: typically 2.0
    - Windows 150% DPI: 1.5
    - Linux HiDPI (Wayland/X11): reads GDK_SCALE or QT_SCALE_FACTOR
    - Falls back to 1.0 on failure or unknown platforms.
    """
    system = platform.system()

    # macOS: use AppKit to query the main screen backing scale factor
    if system == "Darwin":
        try:
            from AppKit import NSScreen  # type: ignore[import-untyped]

            return float(NSScreen.mainScreen().backingScaleFactor())
        except Exception:
            return 1.0

    # Windows: use shcore to get the monitor scale factor
    if system == "Windows":
        try:
            import ctypes

            return ctypes.windll.shcore.GetScaleFactorForDevice(0) / 100.0  # type: ignore[attr-defined]
        except Exception:
            return 1.0

    # Linux: check common env vars set by Wayland/X11 compositors and toolkits
    if system == "Linux":
        try:
            gdk = os.environ.get("GDK_SCALE")
            if gdk:
                return float(gdk)
            qt = os.environ.get("QT_SCALE_FACTOR")
            if qt:
                return float(qt)
        except (ValueError, TypeError):
            pass
        return 1.0

    return 1.0


def get_display_scale() -> float:
    """Return the OS display scale factor (public API).

    This is a convenience wrapper around _get_display_scale() for use by
    external code (e.g. MCP servers) that need to know the current DPI scale.
    """
    return _get_display_scale()


def screenshot() -> Image.Image:
    """Capture a screenshot of all monitors and return a PIL Image.

    The returned image is normalized to logical pixels so that its coordinate
    space matches pynput mouse input (which operates in logical coordinates).

    Tries PIL.ImageGrab first (works on Windows and macOS).
    Falls back to mss if ImageGrab fails (better Linux / multi-monitor support).
    """
    # Try PIL.ImageGrab (Windows + macOS)
    try:
        from PIL import ImageGrab

        img = ImageGrab.grab(all_screens=True)
        if isinstance(img, Image.Image):
            scale = _get_display_scale()
            if scale > 1.0:
                img = img.resize(
                    (int(img.width / scale), int(img.height / scale)),
                    Image.LANCZOS,
                )
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
            img = Image.frombytes("RGB", sct_img.size, sct_img.bgra, "raw", "BGRX")
            scale = _get_display_scale()
            if scale > 1.0:
                img = img.resize(
                    (int(img.width / scale), int(img.height / scale)),
                    Image.LANCZOS,
                )
            return img
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
