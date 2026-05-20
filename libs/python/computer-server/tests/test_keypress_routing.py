"""Tests for DirectComputer.keypress() routing logic.

Verifies that single printable characters route through type_text()
(layout-independent) while named keys route through press_key() and
multi-part combos route through hotkey().
"""

import asyncio
import unittest
from enum import Enum
from typing import List, Union
from unittest.mock import AsyncMock


# Inline Key enum so the test runs without installing the computer package.
# Values match computer.interface.models.Key exactly.
class Key(Enum):
    PAGE_DOWN = "pagedown"
    PAGE_UP = "pageup"
    HOME = "home"
    END = "end"
    LEFT = "left"
    RIGHT = "right"
    UP = "up"
    DOWN = "down"
    RETURN = "enter"
    ENTER = "enter"
    ESCAPE = "esc"
    ESC = "esc"
    TAB = "tab"
    SPACE = "space"
    BACKSPACE = "backspace"
    DELETE = "del"
    ALT = "alt"
    CTRL = "ctrl"
    SHIFT = "shift"
    WIN = "win"
    COMMAND = "command"
    OPTION = "option"
    F1 = "f1"
    F2 = "f2"
    F3 = "f3"
    F4 = "f4"
    F5 = "f5"
    F6 = "f6"
    F7 = "f7"
    F8 = "f8"
    F9 = "f9"
    F10 = "f10"
    F11 = "f11"
    F12 = "f12"

    @classmethod
    def from_string(cls, key: str) -> "Key | str":
        key_mapping = {
            "page_down": cls.PAGE_DOWN, "page down": cls.PAGE_DOWN, "pagedown": cls.PAGE_DOWN,
            "page_up": cls.PAGE_UP, "page up": cls.PAGE_UP, "pageup": cls.PAGE_UP,
            "return": cls.RETURN, "enter": cls.ENTER,
            "escape": cls.ESCAPE, "esc": cls.ESC,
            "delete": cls.DELETE, "del": cls.DELETE,
            "ctrl": cls.CTRL, "control": cls.CTRL,
            "alt": cls.ALT, "option": cls.OPTION,
            "shift": cls.SHIFT, "command": cls.COMMAND, "cmd": cls.COMMAND,
            "win": cls.WIN, "super": cls.WIN, "meta": cls.WIN,
            "backspace": cls.BACKSPACE, "space": cls.SPACE,
            "tab": cls.TAB, "home": cls.HOME, "end": cls.END,
            "left": cls.LEFT, "right": cls.RIGHT, "up": cls.UP, "down": cls.DOWN,
            "f1": cls.F1, "f2": cls.F2, "f3": cls.F3, "f4": cls.F4,
            "f5": cls.F5, "f6": cls.F6, "f7": cls.F7, "f8": cls.F8,
            "f9": cls.F9, "f10": cls.F10, "f11": cls.F11, "f12": cls.F12,
        }
        lower = key.lower()
        if lower in key_mapping:
            return key_mapping[lower]
        try:
            return cls(lower)
        except ValueError:
            return key


class FakeAutomationHandler:
    def __init__(self):
        self.type_text = AsyncMock()
        self.press_key = AsyncMock()
        self.hotkey = AsyncMock()


async def keypress_under_test(auto, keys: Union[List[str], str]) -> None:
    """Extracted keypress logic matching DirectComputer.keypress()."""
    def normalize_key(key: str) -> str:
        result = Key.from_string(key)
        if isinstance(result, Key):
            return result.value
        else:
            return key.lower() if len(key) > 1 else key

    if isinstance(keys, str):
        parts = keys.replace("-", "+").split("+") if len(keys) > 1 else [keys]
    else:
        parts = keys

    parts = [normalize_key(p) for p in parts]

    if len(parts) == 1:
        key = parts[0]
        if len(key) == 1 and key.isprintable():
            await auto.type_text(key)
        else:
            await auto.press_key(key)
    else:
        await auto.hotkey(parts)


class TestKeypressRouting(unittest.TestCase):

    def setUp(self):
        self.auto = FakeAutomationHandler()

    def _run(self, keys):
        asyncio.run(keypress_under_test(self.auto, keys))

    # --- Printable single characters → type_text ---

    def test_lowercase_letter(self):
        self._run("a")
        self.auto.type_text.assert_awaited_once_with("a")
        self.auto.press_key.assert_not_awaited()

    def test_uppercase_letter(self):
        self._run("Z")
        self.auto.type_text.assert_awaited_once_with("Z")

    def test_digit(self):
        self._run("5")
        self.auto.type_text.assert_awaited_once_with("5")

    def test_slash(self):
        self._run("/")
        self.auto.type_text.assert_awaited_once_with("/")

    def test_dot(self):
        self._run(".")
        self.auto.type_text.assert_awaited_once_with(".")

    def test_space_char(self):
        self._run(" ")
        self.auto.type_text.assert_awaited_once_with(" ")

    # --- Named keys → press_key ---

    def test_enter(self):
        self._run("enter")
        self.auto.press_key.assert_awaited_once_with("enter")
        self.auto.type_text.assert_not_awaited()

    def test_return(self):
        self._run("return")
        self.auto.press_key.assert_awaited_once_with("enter")

    def test_tab(self):
        self._run("tab")
        self.auto.press_key.assert_awaited_once_with("tab")

    def test_escape(self):
        self._run("escape")
        self.auto.press_key.assert_awaited_once_with("esc")

    def test_f1(self):
        self._run("f1")
        self.auto.press_key.assert_awaited_once_with("f1")

    def test_left_arrow(self):
        self._run("left")
        self.auto.press_key.assert_awaited_once_with("left")

    def test_backspace(self):
        self._run("backspace")
        self.auto.press_key.assert_awaited_once_with("backspace")

    def test_space_named(self):
        self._run("space")
        self.auto.press_key.assert_awaited_once_with("space")

    # --- Raw control characters → press_key ---

    def test_raw_tab_char(self):
        self._run("\t")
        self.auto.press_key.assert_awaited_once()
        self.auto.type_text.assert_not_awaited()

    def test_raw_newline_char(self):
        self._run("\n")
        self.auto.press_key.assert_awaited_once()
        self.auto.type_text.assert_not_awaited()

    # --- Multi-part combos → hotkey ---

    def test_cmd_c(self):
        self._run("cmd+c")
        self.auto.hotkey.assert_awaited_once()
        self.auto.press_key.assert_not_awaited()
        self.auto.type_text.assert_not_awaited()

    def test_cmd_shift_g(self):
        self._run("cmd+shift+g")
        self.auto.hotkey.assert_awaited_once()

    def test_list_input(self):
        self._run(["ctrl", "a"])
        self.auto.hotkey.assert_awaited_once()


if __name__ == "__main__":
    unittest.main()
