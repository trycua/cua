"""cua-auto — cross-platform automation library.

Provides a synchronous, pyautogui-style API for mouse, keyboard, screen,
window, clipboard, and shell operations.

Usage::

    import cua_auto.mouse as mouse
    import cua_auto.keyboard as keyboard
    import cua_auto.screen as screen
    import cua_auto.window as window
    import cua_auto.clipboard as clipboard
    import cua_auto.shell as shell

    mouse.click(100, 200)
    keyboard.hotkey(["ctrl", "c"])
    img = screen.screenshot()
    title = window.get_active_window_title()
"""

__version__ = "0.1.1"

# terminal and shell have no display dependency — always safe to import.
from cua_auto import shell, terminal

# These modules require a display server (pynput, PIL, pywinctl, pyperclip).
# Guard them so that headless environments (CI without X, computer-server inside
# a container) can still import cua_auto.terminal / cua_auto.shell without error.
try:
    from cua_auto import clipboard, keyboard, mouse, screen, window
except ImportError:
    pass

__all__ = ["mouse", "keyboard", "screen", "window", "clipboard", "shell", "terminal"]
