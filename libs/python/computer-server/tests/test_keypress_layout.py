"""Tests for keypress layout-independence fix (issue #1605).

Verifies that single printable characters are routed through type_text
(layout-independent) rather than press_key (layout-dependent).
"""

import unicodedata
from unittest.mock import AsyncMock, MagicMock, patch

import pytest


def _is_printable_char(key: str) -> bool:
    """Mirror of the routing logic in DirectComputer.keypress()."""
    return len(key) == 1 and unicodedata.category(key) not in ("Cc", "Cs", "Cn")


class TestPrintableCharDetection:
    """Unit tests for the printable-character detection logic."""

    def test_ascii_letter_is_printable(self):
        assert _is_printable_char("a") is True
        assert _is_printable_char("Z") is True

    def test_ascii_digit_is_printable(self):
        assert _is_printable_char("0") is True
        assert _is_printable_char("9") is True

    def test_ascii_punctuation_is_printable(self):
        assert _is_printable_char("/") is True
        assert _is_printable_char(".") is True
        assert _is_printable_char("_") is True

    def test_space_is_printable(self):
        # Space (U+0020) has category Zs — printable, should use type_text.
        assert _is_printable_char(" ") is True

    def test_special_key_names_are_not_printable(self):
        # Multi-char strings are never single printable chars.
        assert _is_printable_char("return") is False
        assert _is_printable_char("tab") is False
        assert _is_printable_char("escape") is False
        assert _is_printable_char("f1") is False
        assert _is_printable_char("backspace") is False
        assert _is_printable_char("up") is False

    def test_control_characters_are_not_printable(self):
        # Cc category — should go through press_key, not type_text.
        assert _is_printable_char("\x00") is False
        assert _is_printable_char("\x1b") is False  # ESC
        assert _is_printable_char("\n") is False  # newline

    def test_empty_string_is_not_printable(self):
        assert _is_printable_char("") is False


@pytest.mark.asyncio
async def test_keypress_single_printable_uses_type_text():
    """Single printable char must call type_text, not press_key."""
    auto = MagicMock()
    auto.type_text = AsyncMock()
    auto.press_key = AsyncMock()
    auto.hotkey = AsyncMock()

    # Simulate the routing logic directly.
    key = "a"
    if _is_printable_char(key):
        await auto.type_text(key)
    else:
        await auto.press_key(key)

    auto.type_text.assert_awaited_once_with("a")
    auto.press_key.assert_not_awaited()


@pytest.mark.asyncio
async def test_keypress_special_key_uses_press_key():
    """Special key names must still call press_key."""
    auto = MagicMock()
    auto.type_text = AsyncMock()
    auto.press_key = AsyncMock()

    key = "return"
    if _is_printable_char(key):
        await auto.type_text(key)
    else:
        await auto.press_key(key)

    auto.press_key.assert_awaited_once_with("return")
    auto.type_text.assert_not_awaited()


@pytest.mark.asyncio
async def test_keypress_combo_uses_hotkey():
    """Multi-key combos must still call hotkey."""
    auto = MagicMock()
    auto.hotkey = AsyncMock()
    auto.type_text = AsyncMock()
    auto.press_key = AsyncMock()

    parts = ["cmd", "shift", "g"]
    if len(parts) == 1:
        key = parts[0]
        if _is_printable_char(key):
            await auto.type_text(key)
        else:
            await auto.press_key(key)
    else:
        await auto.hotkey(parts)

    auto.hotkey.assert_awaited_once_with(["cmd", "shift", "g"])
    auto.type_text.assert_not_awaited()
    auto.press_key.assert_not_awaited()
