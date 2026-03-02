"""Cross-platform mouse control via pynput."""

from typing import List, Optional, Tuple

from pynput.mouse import Button as _Button
from pynput.mouse import Controller as _MouseController

_mouse = _MouseController()


def _map_button(button: str) -> _Button:
    """Map a button name string to a pynput Button."""
    b = (button or "left").lower()
    if b == "right":
        return _Button.right
    if b == "middle":
        return _Button.middle
    return _Button.left


# ── Basic clicks ──────────────────────────────────────────────────────────────


def click(x: int, y: int, button: str = "left") -> None:
    """Single click at (x, y)."""
    _mouse.position = (x, y)
    _mouse.click(_map_button(button), 1)


def right_click(x: int, y: int) -> None:
    """Right-click at (x, y)."""
    click(x, y, "right")


def double_click(x: int, y: int) -> None:
    """Double left-click at (x, y)."""
    _mouse.position = (x, y)
    _mouse.click(_Button.left, 2)


# ── Cursor movement ───────────────────────────────────────────────────────────


def move_to(x: int, y: int) -> None:
    """Move cursor to (x, y) without clicking."""
    _mouse.position = (x, y)


# ── Mouse button hold / release ───────────────────────────────────────────────


def mouse_down(x: Optional[int] = None, y: Optional[int] = None, button: str = "left") -> None:
    """Press and hold a mouse button, optionally moving to (x, y) first."""
    if x is not None and y is not None:
        _mouse.position = (x, y)
    _mouse.press(_map_button(button))


def mouse_up(x: Optional[int] = None, y: Optional[int] = None, button: str = "left") -> None:
    """Release a mouse button, optionally moving to (x, y) first."""
    if x is not None and y is not None:
        _mouse.position = (x, y)
    _mouse.release(_map_button(button))


# ── Drag ──────────────────────────────────────────────────────────────────────


def drag(
    start_x: int,
    start_y: int,
    end_x: int,
    end_y: int,
    button: str = "left",
) -> None:
    """Drag from (start_x, start_y) to (end_x, end_y)."""
    btn = _map_button(button)
    _mouse.position = (start_x, start_y)
    _mouse.press(btn)
    _mouse.position = (end_x, end_y)
    _mouse.release(btn)


def drag_to(x: int, y: int, button: str = "left") -> None:
    """Drag from the current cursor position to (x, y)."""
    btn = _map_button(button)
    _mouse.press(btn)
    _mouse.position = (x, y)
    _mouse.release(btn)


def drag_path(path: List[Tuple[int, int]], button: str = "left") -> None:
    """Drag through a sequence of (x, y) points."""
    if not path:
        return
    btn = _map_button(button)
    _mouse.position = path[0]
    _mouse.press(btn)
    for x, y in path[1:]:
        _mouse.position = (x, y)
    _mouse.release(btn)


# ── Scroll ────────────────────────────────────────────────────────────────────


def scroll(dx: int, dy: int) -> None:
    """Scroll by (dx, dy). Positive dy = scroll up."""
    _mouse.scroll(dx, dy)


def scroll_up(clicks: int = 3) -> None:
    """Scroll up by *clicks* notches."""
    _mouse.scroll(0, abs(clicks))


def scroll_down(clicks: int = 3) -> None:
    """Scroll down by *clicks* notches."""
    _mouse.scroll(0, -abs(clicks))


def scroll_left(clicks: int = 3) -> None:
    """Scroll left by *clicks* notches."""
    _mouse.scroll(-abs(clicks), 0)


def scroll_right(clicks: int = 3) -> None:
    """Scroll right by *clicks* notches."""
    _mouse.scroll(abs(clicks), 0)


# ── Position ──────────────────────────────────────────────────────────────────


def position() -> Tuple[int, int]:
    """Return the current cursor position as (x, y)."""
    x, y = _mouse.position
    return int(x), int(y)
