"""
Linux implementation of automation and accessibility handlers.

This implementation uses pynput for GUI automation. For screenshots and screen size,
it uses PIL's ImageGrab (works with X11/Xvfb) and provides simulated fallbacks where needed.
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

from PIL import Image, ImageGrab

# Configure logger
logger = logging.getLogger(__name__)


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
                           Returns (0, 0) if cursor position cannot be determined.
        """
        try:
            # Use pynput mouse controller
            from pynput.mouse import Controller as MouseController

            m = MouseController()
            x, y = m.position
            return int(x), int(y)
        except Exception as e:
            logger.warning(f"Failed to get cursor position: {e}")
            logger.info("Getting cursor position (simulated)")
            return 0, 0

    def get_screen_size(self) -> Tuple[int, int]:
        """Get the screen size.

        Returns:
            Tuple[int, int]: The width and height of the screen in pixels.
                           Returns (1920, 1080) if screen size cannot be determined.
        """
        try:
            img = ImageGrab.grab()
            return img.width, img.height
        except Exception as e:
            logger.warning(f"Failed to get screen size via ImageGrab: {e}")
            logger.info("Getting screen size (simulated)")
            return 1920, 1080


class LinuxAutomationHandler(BaseAutomationHandler):
    """Linux implementation of automation handler using pynput."""

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
                self.mouse.position = (x, y)
            from pynput.mouse import Button

            btn = getattr(Button, button if button in ["left", "right", "middle"] else "left")
            self.mouse.press(btn)
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
                self.mouse.position = (x, y)
            from pynput.mouse import Button

            btn = getattr(Button, button if button in ["left", "right", "middle"] else "left")
            self.mouse.release(btn)
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
            self.mouse.position = (x, y)
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
            from pynput.mouse import Button

            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(Button.left, 1)
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
            from pynput.mouse import Button

            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(Button.right, 1)
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
            from pynput.mouse import Button

            if x is not None and y is not None:
                self.mouse.position = (x, y)
            self.mouse.click(Button.left, 2)
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
            from pynput.mouse import Button

            if x is not None and y is not None:
                self.mouse.position = (x, y)
            btn = getattr(Button, button if button in ["left", "right", "middle"] else "left")
            self.mouse.click(btn, 1)
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
            from pynput.mouse import Button

            btn = getattr(Button, button if button in ["left", "right", "middle"] else "left")
            self.mouse.press(btn)
            self.mouse.position = (x, y)
            self.mouse.release(btn)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag(
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
            from pynput.mouse import Button

            if not path:
                return {"success": False, "error": "Path is empty"}
            btn = getattr(Button, button if button in ["left", "right", "middle"] else "left")
            self.mouse.position = path[0]
            for x, y in path[1:]:
                self.mouse.press(btn)
                self.mouse.position = (x, y)
                self.mouse.release(btn)
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
            from pynput.keyboard import Key

            k = getattr(Key, key) if hasattr(Key, key) else (key if len(key) == 1 else None)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.press(k)
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
            from pynput.keyboard import Key

            k = getattr(Key, key) if hasattr(Key, key) else (key if len(key) == 1 else None)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.release(k)
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
            from pynput.keyboard import Key

            k = getattr(Key, key) if hasattr(Key, key) else (key if len(key) == 1 else None)
            if k is None:
                return {"success": False, "error": f"Unknown key: {key}"}
            self.keyboard.press(k)
            self.keyboard.release(k)
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
            from pynput.keyboard import Key

            seq = []
            for k in keys:
                kk = getattr(Key, k) if hasattr(Key, k) else (k if len(k) == 1 else None)
                if kk is None:
                    return {"success": False, "error": f"Unknown key in hotkey: {k}"}
                seq.append(kk)
            for k in seq[:-1]:
                self.keyboard.press(k)
            last = seq[-1]
            self.keyboard.press(last)
            self.keyboard.release(last)
            for k in reversed(seq[:-1]):
                self.keyboard.release(k)
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
            self.mouse.scroll(0, -abs(clicks))
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
            self.mouse.scroll(0, abs(clicks))
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
        """Get the size of the screen.

        Returns:
            Dict[str, Any]: A dictionary containing success status and screen dimensions,
                           or error message if failed.
        """
        try:
            img = ImageGrab.grab()
            return {"success": True, "size": {"width": img.width, "height": img.height}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_cursor_position(self) -> Dict[str, Any]:
        """Get the current position of the cursor.

        Returns:
            Dict[str, Any]: A dictionary containing success status and cursor coordinates,
                           or error message if failed.
        """
        try:
            from pynput.mouse import Controller as MouseController

            m = MouseController()
            x, y = m.position
            return {"success": True, "position": {"x": int(x), "y": int(y)}}
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
