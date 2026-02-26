"""Cross-platform window management via pywinctl."""

import os
import platform
import subprocess
import webbrowser
from typing import Any, List, Optional, Tuple

try:
    import pywinctl as _pwc  # type: ignore[import-untyped]
except Exception:
    _pwc = None  # type: ignore[assignment]


def _require_pwc() -> Any:
    if _pwc is None:
        raise RuntimeError("pywinctl is not available. Install it with: pip install pywinctl")
    return _pwc


def _get_by_handle(handle: Any) -> Optional[Any]:
    """Find a pywinctl window object by its native handle."""
    pwc = _require_pwc()
    try:
        for w in pwc.getAllWindows():
            if str(w.getHandle()) == str(handle):
                return w
    except Exception:
        pass
    return None


# ── Active window ─────────────────────────────────────────────────────────────


def get_active_window() -> Optional[Any]:
    """Return the pywinctl window object for the currently focused window."""
    pwc = _require_pwc()
    return pwc.getActiveWindow()


def get_active_window_title() -> str:
    """Return the title of the currently focused window, or 'Desktop'."""
    try:
        win = get_active_window()
        if win:
            return win.title or "Desktop"
    except Exception:
        pass
    return "Desktop"


def get_active_window_handle() -> Optional[str]:
    """Return the native handle of the active window as a string, or None."""
    try:
        win = get_active_window()
        if win:
            return str(win.getHandle())
    except Exception:
        pass
    return None


# ── Window lookup ─────────────────────────────────────────────────────────────


def get_windows_with_title(title: str) -> List[str]:
    """Return a list of native handle strings for windows whose title contains *title*."""
    pwc = _require_pwc()
    try:
        wins = pwc.getWindowsWithTitle(title, condition=pwc.Re.CONTAINS, flags=pwc.Re.IGNORECASE)
        return [str(w.getHandle()) for w in wins]
    except Exception:
        return []


def get_window_name(handle: Any) -> Optional[str]:
    """Return the title of the window with the given handle, or None."""
    w = _get_by_handle(handle)
    return w.title if w else None


def get_window_size(handle: Any) -> Optional[Tuple[int, int]]:
    """Return (width, height) of the window, or None if not found."""
    w = _get_by_handle(handle)
    if not w:
        return None
    width, height = w.size
    return int(width), int(height)


def get_window_position(handle: Any) -> Optional[Tuple[int, int]]:
    """Return (x, y) position of the window, or None if not found."""
    w = _get_by_handle(handle)
    if not w:
        return None
    x, y = w.position
    return int(x), int(y)


# ── Window actions ────────────────────────────────────────────────────────────


def activate_window(handle: Any) -> bool:
    """Bring the window to the foreground and give it focus."""
    w = _get_by_handle(handle)
    if not w:
        return False
    return bool(w.activate())


def minimize_window(handle: Any) -> bool:
    """Minimize the window."""
    w = _get_by_handle(handle)
    if not w:
        return False
    return bool(w.minimize())


def maximize_window(handle: Any) -> bool:
    """Maximize the window."""
    w = _get_by_handle(handle)
    if not w:
        return False
    return bool(w.maximize())


def close_window(handle: Any) -> bool:
    """Close the window."""
    w = _get_by_handle(handle)
    if not w:
        return False
    return bool(w.close())


def set_window_size(handle: Any, width: int, height: int) -> bool:
    """Resize the window to (width, height)."""
    w = _get_by_handle(handle)
    if not w:
        return False
    return bool(w.resizeTo(int(width), int(height)))


def set_window_position(handle: Any, x: int, y: int) -> bool:
    """Move the window to (x, y)."""
    w = _get_by_handle(handle)
    if not w:
        return False
    return bool(w.moveTo(int(x), int(y)))


# ── Open / launch ─────────────────────────────────────────────────────────────


def open(target: str) -> bool:  # noqa: A001
    """Open a URL or file path with the default application.

    URLs are opened in the default browser. Files are opened with the OS
    default handler (``open`` on macOS, ``xdg-open`` on Linux,
    ``os.startfile`` on Windows).
    """
    if target.startswith("http://") or target.startswith("https://"):
        return bool(webbrowser.open(target))

    sys = platform.system().lower()
    if sys == "darwin":
        subprocess.Popen(["open", target])
    elif sys == "linux":
        subprocess.Popen(["xdg-open", target])
    elif sys == "windows":
        os.startfile(target)  # type: ignore[attr-defined]
    else:
        raise RuntimeError(f"Unsupported OS: {sys}")
    return True


def launch(app: str, args: Optional[List[str]] = None) -> int:
    """Launch an application and return its PID.

    If *args* is given, the app is launched with those arguments.
    Otherwise the command string is passed to the system shell, allowing
    strings like ``"libreoffice --writer"``.
    """
    if args:
        proc = subprocess.Popen([app, *args])
    else:
        proc = subprocess.Popen(app, shell=True)
    return proc.pid
