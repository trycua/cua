"""Cross-platform keyboard control via pynput."""

from typing import List, Optional, Union

from pynput.keyboard import Controller as _KBController
from pynput.keyboard import Key as _Key

_kb = _KBController()

# Unified key name → pynput Key map (sourced from computer-server/handlers/windows.py)
_SPECIAL: dict = {
    "enter": _Key.enter,
    "return": _Key.enter,
    "esc": _Key.esc,
    "escape": _Key.esc,
    "space": _Key.space,
    "tab": _Key.tab,
    "backspace": _Key.backspace,
    "delete": _Key.delete,
    "home": _Key.home,
    "end": _Key.end,
    "pageup": _Key.page_up,
    "page_up": _Key.page_up,
    "pagedown": _Key.page_down,
    "page_down": _Key.page_down,
    "up": _Key.up,
    "down": _Key.down,
    "left": _Key.left,
    "right": _Key.right,
    "shift": _Key.shift,
    "shift_l": _Key.shift_l,
    "shift_r": _Key.shift_r,
    "ctrl": _Key.ctrl,
    "ctrl_l": _Key.ctrl_l,
    "ctrl_r": _Key.ctrl_r,
    "control": _Key.ctrl,
    "alt": _Key.alt,
    "alt_l": _Key.alt_l,
    "alt_r": _Key.alt_r,
    "cmd": _Key.cmd,
    "win": _Key.cmd,
    "super": _Key.cmd,
    "meta": _Key.cmd,
    "capslock": _Key.caps_lock,
    "caps_lock": _Key.caps_lock,
    "insert": _Key.insert,
    "print_screen": _Key.print_screen,
    "pause": _Key.pause,
    "num_lock": _Key.num_lock,
    "scroll_lock": _Key.scroll_lock,
    "f1": _Key.f1,
    "f2": _Key.f2,
    "f3": _Key.f3,
    "f4": _Key.f4,
    "f5": _Key.f5,
    "f6": _Key.f6,
    "f7": _Key.f7,
    "f8": _Key.f8,
    "f9": _Key.f9,
    "f10": _Key.f10,
    "f11": _Key.f11,
    "f12": _Key.f12,
    "f13": _Key.f13,
    "f14": _Key.f14,
    "f15": _Key.f15,
    "f16": _Key.f16,
    "f17": _Key.f17,
    "f18": _Key.f18,
    "f19": _Key.f19,
    "f20": _Key.f20,
}


def _resolve(key: str) -> Optional[Union[str, _Key]]:
    """Resolve a key string to a pynput Key or single character."""
    if not key:
        return None
    lk = key.lower()
    if lk in _SPECIAL:
        return _SPECIAL[lk]
    if len(key) == 1:
        return key
    return None


# ── Key press / release ───────────────────────────────────────────────────────


def key_down(key: str) -> None:
    """Press and hold a key."""
    k = _resolve(key)
    if k is None:
        raise ValueError(f"Unknown key: {key!r}")
    _kb.press(k)


def key_up(key: str) -> None:
    """Release a key."""
    k = _resolve(key)
    if k is None:
        raise ValueError(f"Unknown key: {key!r}")
    _kb.release(k)


def press_key(key: str) -> None:
    """Press and immediately release a key."""
    k = _resolve(key)
    if k is None:
        raise ValueError(f"Unknown key: {key!r}")
    _kb.press(k)
    _kb.release(k)


# ── Text typing ───────────────────────────────────────────────────────────────


def type_text(text: str) -> None:
    """Type a string of text (supports Unicode)."""
    _kb.type(text)


# ── Hotkeys ───────────────────────────────────────────────────────────────────


def hotkey(keys: List[str]) -> None:
    """Press a combination of keys, e.g. ['ctrl', 'c'].

    Modifiers are held while the last key is tapped, then released in reverse
    order (standard hotkey behaviour).
    """
    resolved = [_resolve(k) for k in keys]
    if any(k is None for k in resolved):
        bad = [k for k, r in zip(keys, resolved) if r is None]
        raise ValueError(f"Unknown keys in hotkey: {bad}")

    seq: List[Union[str, _Key]] = [k for k in resolved if k is not None]
    if not seq:
        return

    # Hold all modifiers except the last key
    for k in seq[:-1]:
        _kb.press(k)
    # Tap the action key
    last = seq[-1]
    _kb.press(last)
    _kb.release(last)
    # Release modifiers in reverse order
    for k in reversed(seq[:-1]):
        _kb.release(k)
