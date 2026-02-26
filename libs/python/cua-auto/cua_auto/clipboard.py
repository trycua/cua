"""Cross-platform clipboard access via pyperclip."""

import pyperclip as _pyperclip


def get() -> str:
    """Return the current clipboard text content."""
    return _pyperclip.paste()


def set(text: str) -> None:
    """Set the clipboard text content."""
    _pyperclip.copy(text)
