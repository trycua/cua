"""
Linux implementation of automation and accessibility handlers.

This implementation attempts to use pyautogui for GUI automation when available.
If running in a headless environment without X11, it will fall back to simulated responses.
To use GUI automation in a headless environment:
1. Install Xvfb: sudo apt-get install xvfb
2. Run with virtual display: xvfb-run python -m computer_server
"""

import asyncio
import base64
import json
import logging
import os
import subprocess
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

# Configure logger
logger = logging.getLogger(__name__)

# Try to import pyautogui, but don't fail if it's not available
# This allows the server to run in headless environments
try:
    import pyautogui

    pyautogui.FAILSAFE = False

    logger.info("pyautogui successfully imported, GUI automation available")
except Exception as e:
    logger.warning(f"pyautogui import failed: {str(e)}. GUI operations will be simulated.")

from pynput.keyboard import Controller as KeyboardController
from pynput.keyboard import Key
from pynput.mouse import Button
from pynput.mouse import Controller as MouseController

from .base import BaseAccessibilityHandler, BaseAutomationHandler


class LinuxAccessibilityHandler(BaseAccessibilityHandler):
    """Linux implementation of accessibility handler."""

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree of the current window.

        Returns:
            Dict[str, Any]: A dictionary containing success status and a simulated tree structure
                           since Linux doesn't have equivalent accessibility API like macOS.
        """
        # Linux doesn't have equivalent accessibility API like macOS
        # Return a minimal dummy tree
        logger.info(
            "Getting accessibility tree (simulated, no accessibility API available on Linux)"
        )
        return {
            "success": True,
            "tree": {
                "role": "Window",
                "title": "Linux Window",
                "position": {"x": 0, "y": 0},
                "size": {"width": 1920, "height": 1080},
                "children": [],
            },
        }

    async def find_element(
        self, role: Optional[str] = None, title: Optional[str] = None, value: Optional[str] = None
    ) -> Dict[str, Any]:
        """Find an element in the accessibility tree by criteria.

        Args:
            role: The role of the element to find.
            title: The title of the element to find.
            value: The value of the element to find.

        Returns:
            Dict[str, Any]: A dictionary indicating that element search is not supported on Linux.
        """
        logger.info(
            f"Finding element with role={role}, title={title}, value={value} (not supported on Linux)"
        )
        return {"success": False, "message": "Element search not supported on Linux"}

    def get_cursor_position(self) -> Tuple[int, int]:
        """Get the current cursor position.

        Returns:
            Tuple[int, int]: The x and y coordinates of the cursor position.
                           Returns (0, 0) if pyautogui is not available.
        """
        try:
            pos = pyautogui.position()
            return pos.x, pos.y
        except Exception as e:
            logger.warning(f"Failed to get cursor position with pyautogui: {e}")

        logger.info("Getting cursor position (simulated)")
        return 0, 0

    def get_screen_size(self) -> Tuple[int, int]:
        """Get the screen size.

        Returns:
            Tuple[int, int]: The width and height of the screen in pixels.
                           Returns (1920, 1080) if pyautogui is not available.
        """
        try:
            size = pyautogui.size()
            return size.width, size.height
        except Exception as e:
            logger.warning(f"Failed to get screen size with pyautogui: {e}")

        logger.info("Getting screen size (simulated)")
        return 1920, 1080


class LinuxAutomationHandler(BaseAutomationHandler):
    """Linux implementation of automation handler using pyautogui."""

    keyboard = KeyboardController()
    mouse = MouseController()

    # Mouse Actions
    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Press and hold a mouse button at the specified coordinates.

        Args:
            x: The x coordinate to move to before pressing. If None, uses current position.
            y: The y coordinate to move to before pressing. If None, uses current position.
            button: The mouse button to press ("left", "right", or "middle").

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.mouseDown(button=button)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Release a mouse button at the specified coordinates.

        Args:
            x: The x coordinate to move to before releasing. If None, uses current position.
            y: The y coordinate to move to before releasing. If None, uses current position.
            button: The mouse button to release ("left", "right", or "middle").

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.mouseUp(button=button)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        """Move the cursor to the specified coordinates.

        Args:
            x: The x coordinate to move to.
            y: The y coordinate to move to.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.moveTo(x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a left mouse click at the specified coordinates.

        Args:
            x: The x coordinate to click at. If None, clicks at current position.
            y: The y coordinate to click at. If None, clicks at current position.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.click()
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a right mouse click at the specified coordinates.

        Args:
            x: The x coordinate to click at. If None, clicks at current position.
            y: The y coordinate to click at. If None, clicks at current position.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.rightClick()
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        """Perform a double click at the specified coordinates.

        Args:
            x: The x coordinate to double click at. If None, clicks at current position.
            y: The y coordinate to double click at. If None, clicks at current position.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.doubleClick(interval=0.1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def click(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Perform a mouse click with the specified button at the given coordinates.

        Args:
            x: The x coordinate to click at. If None, clicks at current position.
            y: The y coordinate to click at. If None, clicks at current position.
            button: The mouse button to click ("left", "right", or "middle").

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.click(button=button)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag from the current position to the specified coordinates.

        Args:
            x: The x coordinate to drag to.
            y: The y coordinate to drag to.
            button: The mouse button to use for dragging.
            duration: The time in seconds to take for the drag operation.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.dragTo(x, y, duration=duration, button=button)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag(
        self, start_x: int, start_y: int, end_x: int, end_y: int, button: str = "left"
    ) -> Dict[str, Any]:
        """Drag from start coordinates to end coordinates.

        Args:
            start_x: The starting x coordinate.
            start_y: The starting y coordinate.
            end_x: The ending x coordinate.
            end_y: The ending y coordinate.
            button: The mouse button to use for dragging.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.moveTo(start_x, start_y)
            pyautogui.dragTo(end_x, end_y, duration=0.5, button=button)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag_path(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag along a path defined by a list of coordinates.

        Args:
            path: A list of (x, y) coordinate tuples defining the drag path.
            button: The mouse button to use for dragging.
            duration: The time in seconds to take for each segment of the drag.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            if not path:
                return {"success": False, "error": "Path is empty"}
            pyautogui.moveTo(*path[0])
            for x, y in path[1:]:
                pyautogui.dragTo(x, y, duration=duration, button=button)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Keyboard Actions
    async def key_down(self, key: str) -> Dict[str, Any]:
        """Press and hold a key.

        Args:
            key: The key to press down.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.keyDown(key)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def key_up(self, key: str) -> Dict[str, Any]:
        """Release a key.

        Args:
            key: The key to release.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.keyUp(key)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def type_text(self, text: str) -> Dict[str, Any]:
        """Type the specified text using the keyboard.

        Args:
            text: The text to type.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            # use pynput for Unicode support
            self.keyboard.type(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def press_key(self, key: str) -> Dict[str, Any]:
        """Press and release a key.

        Args:
            key: The key to press.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.press(key)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        """Press a combination of keys simultaneously.

        Args:
            keys: A list of keys to press together as a hotkey combination.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.hotkey(*keys)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Scrolling Actions
    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        """Scroll the mouse wheel.

        Args:
            x: The horizontal scroll amount.
            y: The vertical scroll amount.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            self.mouse.scroll(x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll down by the specified number of clicks.

        Args:
            clicks: The number of scroll clicks to perform downward.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.scroll(-clicks)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll up by the specified number of clicks.

        Args:
            clicks: The number of scroll clicks to perform upward.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
        """
        try:
            pyautogui.scroll(clicks)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Screen Actions
    async def screenshot(self) -> Dict[str, Any]:
        """Take a screenshot of the current screen.

        Returns:
            Dict[str, Any]: A dictionary containing success status and base64-encoded image data,
                           or error message if failed.
        """
        try:
            from PIL import Image

            screenshot = pyautogui.screenshot()
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
        """Get the size of the screen.

        Returns:
            Dict[str, Any]: A dictionary containing success status and screen dimensions,
                           or error message if failed.
        """
        try:
            size = pyautogui.size()
            return {"success": True, "size": {"width": size.width, "height": size.height}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_cursor_position(self) -> Dict[str, Any]:
        """Get the current position of the cursor.

        Returns:
            Dict[str, Any]: A dictionary containing success status and cursor coordinates,
                           or error message if failed.
        """
        try:
            pos = pyautogui.position()
            return {"success": True, "position": {"x": pos.x, "y": pos.y}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Clipboard Actions
    async def copy_to_clipboard(self) -> Dict[str, Any]:
        """Get the current content of the clipboard.

        Returns:
            Dict[str, Any]: A dictionary containing success status and clipboard content,
                           or error message if failed.
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
            text: The text to copy to the clipboard.

        Returns:
            Dict[str, Any]: A dictionary with success status and error message if failed.
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
            command: The shell command to execute.

        Returns:
            Dict[str, Any]: A dictionary containing success status, stdout, stderr,
                           and return code, or error message if failed.
        """
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
                "stdout": stdout.decode() if stdout else "",
                "stderr": stderr.decode() if stderr else "",
                "return_code": process.returncode,
            }
        except Exception as e:
            return {"success": False, "error": str(e)}
