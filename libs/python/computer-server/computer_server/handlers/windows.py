"""
Windows implementation of automation and accessibility handlers.

This implementation uses pynput for GUI automation and Windows-specific APIs
for accessibility and system operations.
"""

import asyncio
import base64
import functools
import logging
import os
import subprocess
from io import BytesIO
from typing import Any, Callable, Dict, List, Optional, Tuple, TypeVar, Union

F = TypeVar("F", bound=Callable[..., Any])


def require_unlocked_desktop(func: F) -> F:
    """Decorator that checks if the Windows desktop is locked before executing.

    Returns an error response if the desktop is locked, preventing automation
    actions that would silently fail on the Windows Secure Desktop.
    """

    @functools.wraps(func)
    async def wrapper(self: "WindowsAutomationHandler", *args: Any, **kwargs: Any) -> Dict[str, Any]:
        if self.is_desktop_locked():
            return {
                "success": False,
                "error": "Windows desktop is locked. Automation input is blocked by OS security.",
            }
        return await func(self, *args, **kwargs)

    return wrapper  # type: ignore[return-value]

from PIL import Image, ImageGrab
from pynput.keyboard import Controller as KeyboardController
from pynput.keyboard import Key as KBKey
from pynput.mouse import Button as MouseButton
from pynput.mouse import Controller as MouseController

# Configure logger
logger = logging.getLogger(__name__)

# pyautogui removed in favor of pynput

# Try to import Windows-specific modules
try:
    import win32api
    import win32con
    import win32gui

    logger.info("Windows API modules successfully imported")
    WINDOWS_API_AVAILABLE = True
except Exception as e:
    logger.error(
        f"Windows API modules import failed: {str(e)}. Some Windows-specific features will be unavailable."
    )
    WINDOWS_API_AVAILABLE = False

from .base import BaseAccessibilityHandler, BaseAutomationHandler


class WindowsAccessibilityHandler(BaseAccessibilityHandler):
    """Windows implementation of accessibility handler."""

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree of the current window.

        Returns:
            Dict[str, Any]: A dictionary containing the success status and either
                           the accessibility tree or an error message.
                           Structure: {"success": bool, "tree": dict} or
                                    {"success": bool, "error": str}
        """
        if not WINDOWS_API_AVAILABLE:
            return {"success": False, "error": "Windows API not available"}

        try:
            # Get the foreground window
            hwnd = win32gui.GetForegroundWindow()
            if not hwnd:
                return {"success": False, "error": "No foreground window found"}

            # Get window information
            window_text = win32gui.GetWindowText(hwnd)
            rect = win32gui.GetWindowRect(hwnd)

            tree = {
                "role": "Window",
                "title": window_text,
                "position": {"x": rect[0], "y": rect[1]},
                "size": {"width": rect[2] - rect[0], "height": rect[3] - rect[1]},
                "children": [],
            }

            # Enumerate child windows
            def enum_child_proc(hwnd_child, children_list):
                """Callback function to enumerate child windows and collect their information.

                Args:
                    hwnd_child: Handle to the child window being enumerated.
                    children_list: List to append child window information to.

                Returns:
                    bool: True to continue enumeration, False to stop.
                """
                try:
                    child_text = win32gui.GetWindowText(hwnd_child)
                    child_rect = win32gui.GetWindowRect(hwnd_child)
                    child_class = win32gui.GetClassName(hwnd_child)

                    child_info = {
                        "role": child_class,
                        "title": child_text,
                        "position": {"x": child_rect[0], "y": child_rect[1]},
                        "size": {
                            "width": child_rect[2] - child_rect[0],
                            "height": child_rect[3] - child_rect[1],
                        },
                        "children": [],
                    }
                    children_list.append(child_info)
                except Exception as e:
                    logger.debug(f"Error getting child window info: {e}")
                return True

            win32gui.EnumChildWindows(hwnd, enum_child_proc, tree["children"])

            return {"success": True, "tree": tree}

        except Exception as e:
            logger.error(f"Error getting accessibility tree: {e}")
            return {"success": False, "error": str(e)}

    async def find_element(
        self, role: Optional[str] = None, title: Optional[str] = None, value: Optional[str] = None
    ) -> Dict[str, Any]:
        """Find an element in the accessibility tree by criteria.

        Args:
            role (Optional[str]): The role or class name of the element to find.
            title (Optional[str]): The title or text of the element to find.
            value (Optional[str]): The value of the element (not used in Windows implementation).

        Returns:
            Dict[str, Any]: A dictionary containing the success status and either
                           the found element or an error message.
                           Structure: {"success": bool, "element": dict} or
                                    {"success": bool, "error": str}
        """
        if not WINDOWS_API_AVAILABLE:
            return {"success": False, "error": "Windows API not available"}

        try:
            # Find window by title if specified
            if title:
                hwnd = win32gui.FindWindow(None, title)
                if hwnd:
                    rect = win32gui.GetWindowRect(hwnd)
                    return {
                        "success": True,
                        "element": {
                            "role": "Window",
                            "title": title,
                            "position": {"x": rect[0], "y": rect[1]},
                            "size": {"width": rect[2] - rect[0], "height": rect[3] - rect[1]},
                        },
                    }

            # Find window by class name if role is specified
            if role:
                hwnd = win32gui.FindWindow(role, None)
                if hwnd:
                    window_text = win32gui.GetWindowText(hwnd)
                    rect = win32gui.GetWindowRect(hwnd)
                    return {
                        "success": True,
                        "element": {
                            "role": role,
                            "title": window_text,
                            "position": {"x": rect[0], "y": rect[1]},
                            "size": {"width": rect[2] - rect[0], "height": rect[3] - rect[1]},
                        },
                    }

            return {"success": False, "error": "Element not found"}

        except Exception as e:
            logger.error(f"Error finding element: {e}")
            return {"success": False, "error": str(e)}


class WindowsAutomationHandler(BaseAutomationHandler):
    """Windows implementation of automation handler using pynput and Windows APIs."""

    mouse = MouseController()
    keyboard = KeyboardController()

    def is_desktop_locked(self) -> bool:
        try:
            import ctypes
            user32 = ctypes.windll.user32
            hwnd = user32.GetForegroundWindow()
            return hwnd == 0
        except Exception:
            return False

    def _map_button(self, button: str) -> MouseButton:
        """Map a string button name to pynput MouseButton."""
        b = (button or "left").lower()
        if b == "left":
            return MouseButton.left
        if b == "right":
            return MouseButton.right
        if b == "middle":
            return MouseButton.middle
        # default to left
        return MouseButton.left

    def _key_from_string(self, key: str):
        """Convert a key string (e.g., 'enter', 'ctrl', 'a') to pynput Key or char."""
        if not key:
            return None
        lk = key.lower()
        special = {
            "enter": KBKey.enter,
            "return": KBKey.enter,
            "esc": KBKey.esc,
            "escape": KBKey.esc,
            "space": KBKey.space,
            "tab": KBKey.tab,
            "backspace": KBKey.backspace,
            "delete": KBKey.delete,
            "home": KBKey.home,
            "end": KBKey.end,
            "pageup": KBKey.page_up,
            "pagedown": KBKey.page_down,
            "up": KBKey.up,
            "down": KBKey.down,
            "left": KBKey.left,
            "right": KBKey.right,
            "shift": KBKey.shift,
            "ctrl": KBKey.ctrl,
            "control": KBKey.ctrl,
            "alt": KBKey.alt,
            "cmd": KBKey.cmd,
            "win": KBKey.cmd,
            "meta": KBKey.cmd,
            "capslock": KBKey.caps_lock,
            "f1": KBKey.f1,
            "f2": KBKey.f2,
            "f3": KBKey.f3,
            "f4": KBKey.f4,
            "f5": KBKey.f5,
            "f6": KBKey.f6,
            "f7": KBKey.f7,
            "f8": KBKey.f8,
            "f9": KBKey.f9,
            "f10": KBKey.f10,
            "f11": KBKey.f11,
            "f12": KBKey.f12,
        }
        if lk in special:
            return special[lk]
        # single character
        if len(key) == 1:
            return key
        return None

    # Mouse Actions
    @require_unlocked_desktop
    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Press and hold a mouse button at the specified coordinates.

        Args:
            x (Optional[int]): The x-coordinate to move to before pressing. If None, uses current position.
            y (Optional[int]): The y-coordinate to move to before pressing. If None, uses current position.
            button (str): The mouse button to press ("left", "right", or "middle").

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.press(self._map_button(button))
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Release a mouse button at the specified coordinates.

        Args:
            x (Optional[int]): The x-coordinate to move to before releasing. If None, uses current position.
            y (Optional[int]): The y-coordinate to move to before releasing. If None, uses current position.
            button (str): The mouse button to release ("left", "right", or "middle").

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.release(self._map_button(button))
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        """Move the mouse cursor to the specified coordinates.

        Args:
            x (int): The x-coordinate to move to.
            y (int): The y-coordinate to move to.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            self.mouse.position = (x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a left mouse click at the specified coordinates.

        Args:
            x (Optional[int]): The x-coordinate to click at. If None, clicks at current position.
            y (Optional[int]): The y-coordinate to click at. If None, clicks at current position.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(MouseButton.left, 1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a right mouse click at the specified coordinates.

        Args:
            x (Optional[int]): The x-coordinate to click at. If None, clicks at current position.
            y (Optional[int]): The y-coordinate to click at. If None, clicks at current position.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(MouseButton.right, 1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        """Perform a double left mouse click at the specified coordinates.

        Args:
            x (Optional[int]): The x-coordinate to double-click at. If None, clicks at current position.
            y (Optional[int]): The y-coordinate to double-click at. If None, clicks at current position.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(MouseButton.left, 2)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag from the current position to the specified coordinates.

        Args:
            x (int): The x-coordinate to drag to.
            y (int): The y-coordinate to drag to.
            button (str): The mouse button to use for dragging ("left", "right", or "middle").
            duration (float): The time in seconds to take for the drag operation.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            # simple drag implementation
            self.mouse.press(self._map_button(button))
            self.mouse.position = (x, y)
            self.mouse.release(self._map_button(button))
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag the mouse through a series of coordinates.

        Args:
            path (List[Tuple[int, int]]): A list of (x, y) coordinate tuples to drag through.
            button (str): The mouse button to use for dragging ("left", "right", or "middle").
            duration (float): The total time in seconds for the entire drag operation.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            if not path:
                return {"success": False, "error": "Path is empty"}

            # Move to first position
            self.mouse.position = path[0]

            # Drag through all positions
            for x, y in path[1:]:
                self.mouse.press(self._map_button(button))
                self.mouse.position = (x, y)
                self.mouse.release(self._map_button(button))

            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Keyboard Actions
    @require_unlocked_desktop
    async def key_down(self, key: str) -> Dict[str, Any]:
        """Press and hold a keyboard key.

        Args:
            key (str): The key to press down (e.g., 'ctrl', 'shift', 'a').

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            k = self._key_from_string(key)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.press(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def key_up(self, key: str) -> Dict[str, Any]:
        """Release a keyboard key.

        Args:
            key (str): The key to release (e.g., 'ctrl', 'shift', 'a').

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            k = self._key_from_string(key)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.release(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def type_text(self, text: str) -> Dict[str, Any]:
        """Type the specified text.

        Args:
            text (str): The text to type.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            # use pynput for Unicode support
            self.keyboard.type(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def press_key(self, key: str) -> Dict[str, Any]:
        """Press and release a keyboard key.

        Args:
            key (str): The key to press (e.g., 'enter', 'space', 'tab').

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            k = self._key_from_string(key)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.press(k)
            self.keyboard.release(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        """Press a combination of keys simultaneously.

        Args:
            keys (List[str]): The keys to press together (e.g., ['ctrl', 'c'], ['alt', 'tab']).

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            # press keys sequentially while holding modifiers
            resolved = [self._key_from_string(k) for k in keys]
            if any(k is None for k in resolved):
                return {"success": False, "error": "Unknown key in hotkey sequence"}
            seq: List[Union[str, KBKey]] = [k for k in resolved if k is not None]  # type: ignore[assignment]
            if not seq:
                return {"success": False, "error": "Empty hotkey sequence"}
            # hold all except the last
            for k in seq[:-1]:
                self.keyboard.press(k)
            # tap last
            last = seq[-1]
            self.keyboard.press(last)
            self.keyboard.release(last)
            # release modifiers
            for k in reversed(seq[:-1]):
                self.keyboard.release(k)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Scrolling Actions
    @require_unlocked_desktop
    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        """Scroll vertically at the current cursor position.

        Args:
            x (int): Horizontal scroll amount.
            y (int): Vertical scroll amount. Positive values scroll up, negative values scroll down.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            self.mouse.scroll(x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll down by the specified number of clicks.

        Args:
            clicks (int): The number of scroll clicks to perform downward.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            # negative y to scroll down
            self.mouse.scroll(0, -abs(clicks))
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    @require_unlocked_desktop
    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll up by the specified number of clicks.

        Args:
            clicks (int): The number of scroll clicks to perform upward.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            self.mouse.scroll(0, abs(clicks))
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Screen Actions
    @require_unlocked_desktop
    async def screenshot(self) -> Dict[str, Any]:
        """Capture a screenshot of the entire screen.

        Returns:
            Dict[str, Any]: A dictionary containing the success status and either
                           base64-encoded image data or an error message.
                           Structure: {"success": bool, "image_data": str} or
                                    {"success": bool, "error": str}
        """
        try:
            screenshot = ImageGrab.grab()
            if not isinstance(screenshot, Image.Image):
                return {"success": False, "error": "Failed to capture screenshot"}

            buffered = BytesIO()
            screenshot.save(buffered, format="PNG", optimize=True)
            buffered.seek(0)
            image_data = base64.b64encode(buffered.getvalue()).decode()
            return {"success": True, "image_data": image_data}
        except Exception as e:
            return {"success": False, "error": f"Screenshot error: {str(e)}"}

    async def get_screen_size(self) -> Dict[str, Any]:
        """Get the size of the screen in pixels.

        Returns:
            Dict[str, Any]: A dictionary containing the success status and either
                           screen size information or an error message.
                           Structure: {"success": bool, "size": {"width": int, "height": int}} or
                                    {"success": bool, "error": str}
        """
        try:
            if WINDOWS_API_AVAILABLE:
                width = win32api.GetSystemMetrics(win32con.SM_CXSCREEN)
                height = win32api.GetSystemMetrics(win32con.SM_CYSCREEN)
                return {"success": True, "size": {"width": width, "height": height}}
            else:
                # Fallback: use ImageGrab
                img = ImageGrab.grab()
                return {"success": True, "size": {"width": img.width, "height": img.height}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_cursor_position(self) -> Dict[str, Any]:
        """Get the current position of the mouse cursor.

        Returns:
            Dict[str, Any]: A dictionary containing the success status and either
                           cursor position or an error message.
                           Structure: {"success": bool, "position": {"x": int, "y": int}} or
                                    {"success": bool, "error": str}
        """
        try:
            if WINDOWS_API_AVAILABLE:
                pos = win32gui.GetCursorPos()
                return {"success": True, "position": {"x": pos[0], "y": pos[1]}}
            else:
                # Fallback: use pynput controller
                x, y = self.mouse.position
                return {"success": True, "position": {"x": int(x), "y": int(y)}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Clipboard Actions
    async def copy_to_clipboard(self) -> Dict[str, Any]:
        """Get the current content of the clipboard.

        Returns:
            Dict[str, Any]: A dictionary containing the success status and either
                           clipboard content or an error message.
                           Structure: {"success": bool, "content": str} or
                                    {"success": bool, "error": str}
        """
        try:
            import pyperclip

            content = pyperclip.paste()
            return {"success": True, "content": content}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def set_clipboard(self, text: str) -> Dict[str, Any]:
        """Set the clipboard content to the specified text.

        Args:
            text (str): The text to copy to the clipboard.

        Returns:
            Dict[str, Any]: A dictionary with success status and optional error message.
        """
        try:
            import pyperclip

            pyperclip.copy(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Command Execution
    async def run_command(self, command: str) -> Dict[str, Any]:
        """Execute a shell command asynchronously.

        Args:
            command (str): The shell command to execute.

        Returns:
            Dict[str, Any]: A dictionary containing the success status and either
                           command output or an error message.
                           Structure: {"success": bool, "stdout": str, "stderr": str, "return_code": int} or
                                    {"success": bool, "error": str}
        """
        def decode_output(data: bytes) -> str:
            if not data:
                return ""
            encodings = ['utf-8', 'gbk', 'gb2312', 'cp936', 'latin1']
            for enc in encodings:
                try:
                    return data.decode(enc)
                except (UnicodeDecodeError, LookupError):
                    continue
            return data.decode('utf-8', errors='replace')

        try:
            # Create subprocess
            process = await asyncio.create_subprocess_shell(
                command, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
            )
            # Wait for the subprocess to finish
            stdout, stderr = await process.communicate()
            # Return decoded output
            return {
                "success": True,
                "stdout": decode_output(stdout),
                "stderr": decode_output(stderr),

                "return_code": process.returncode,
            }
        except Exception as e:
            return {"success": False, "error": str(e)}
