"""Computer tool for OpenAI."""

import asyncio
import base64
import logging
from typing import Literal, Any, Dict, Optional, List, Union

from computer.computer import Computer

from .base import BaseOpenAITool, ToolError, ToolResult
from ....core.tools.computer import BaseComputerTool

TYPING_DELAY_MS = 12
TYPING_GROUP_SIZE = 50

# Key mapping for special keys
KEY_MAPPING = {
    "enter": "return",
    "backspace": "delete",
    "delete": "forwarddelete",
    "escape": "esc",
    "pageup": "page_up",
    "pagedown": "page_down",
    "arrowup": "up",
    "arrowdown": "down",
    "arrowleft": "left",
    "arrowright": "right",
    "home": "home",
    "end": "end",
    "tab": "tab",
    "space": "space",
    "shift": "shift",
    "control": "control",
    "alt": "alt",
    "meta": "command",
}

Action = Literal[
    "key",
    "type",
    "mouse_move",
    "left_click",
    "right_click",
    "double_click",
    "screenshot",
    "scroll",
    "drag",
]


class ComputerTool(BaseComputerTool, BaseOpenAITool):
    """
    A tool that allows the agent to interact with the screen, keyboard, and mouse of the current computer.
    """

    name: Literal["computer"] = "computer"
    api_type: Literal["computer_use_preview"] = "computer_use_preview"
    width: Optional[int] = None
    height: Optional[int] = None
    display_num: Optional[int] = None
    computer: Computer  # The CUA Computer instance
    logger = logging.getLogger(__name__)

    _screenshot_delay = 1.0  # macOS is generally faster than X11
    _scaling_enabled = True

    def __init__(self, computer: Computer):
        """Initialize the computer tool.

        Args:
            computer: Computer instance
        """
        self.computer = computer
        self.width = None
        self.height = None
        self.logger = logging.getLogger(__name__)

        # Initialize the base computer tool first
        BaseComputerTool.__init__(self, computer)
        # Then initialize the OpenAI tool
        BaseOpenAITool.__init__(self)

        # Additional initialization
        self.width = None  # Will be initialized from computer interface
        self.height = None  # Will be initialized from computer interface
        self.display_num = None

    def to_params(self) -> Dict[str, Any]:
        """Convert tool to API parameters.

        Returns:
            Dictionary with tool parameters
        """
        if self.width is None or self.height is None:
            raise RuntimeError(
                "Screen dimensions not initialized. Call initialize_dimensions() first."
            )
        return {
            "type": self.api_type,
            "display_width": self.width,
            "display_height": self.height,
            "display_number": self.display_num,
        }

    async def initialize_dimensions(self):
        """Initialize screen dimensions from the computer interface."""
        try:
            display_size = await self.computer.interface.get_screen_size()
            self.width = display_size["width"]
            self.height = display_size["height"]
            assert isinstance(self.width, int) and isinstance(self.height, int)
            self.logger.info(f"Initialized screen dimensions to {self.width}x{self.height}")
        except Exception as e:
            # Fall back to defaults if we can't get accurate dimensions
            self.width = 1024
            self.height = 768
            self.logger.warning(
                f"Failed to get screen dimensions, using defaults: {self.width}x{self.height}. Error: {e}"
            )

    async def __call__(
        self,
        *,
        type: str,  # OpenAI uses 'type' instead of 'action'
        text: Optional[str] = None,
        **kwargs,
    ):
        try:
            # Ensure dimensions are initialized
            if self.width is None or self.height is None:
                await self.initialize_dimensions()
                if self.width is None or self.height is None:
                    raise ToolError("Failed to initialize screen dimensions")

            if type == "type":
                if text is None:
                    raise ToolError("text is required for type action")
                return await self.handle_typing(text)
            elif type == "click":
                # Map button to correct action name
                button = kwargs.get("button")
                if button is None:
                    raise ToolError("button is required for click action")
                return await self.handle_click(button, kwargs["x"], kwargs["y"])
            elif type == "keypress":
                # Check for keys in kwargs if text is None
                if text is None:
                    if "keys" in kwargs and isinstance(kwargs["keys"], list):
                        # Pass the keys list directly instead of joining and then splitting
                        return await self.handle_key(kwargs["keys"])
                    else:
                        raise ToolError("Either 'text' or 'keys' is required for keypress action")
                return await self.handle_key(text)
            elif type == "mouse_move":
                if "coordinates" not in kwargs:
                    raise ToolError("coordinates is required for mouse_move action")
                return await self.handle_mouse_move(
                    kwargs["coordinates"][0], kwargs["coordinates"][1]
                )
            elif type == "scroll":
                # Get x, y coordinates directly from kwargs
                x = kwargs.get("x")
                y = kwargs.get("y")
                if x is None or y is None:
                    raise ToolError("x and y coordinates are required for scroll action")
                scroll_x = kwargs.get("scroll_x", 0) // 50
                scroll_y = kwargs.get("scroll_y", 0) // 50
                return await self.handle_scroll(x, y, scroll_x, scroll_y)
            elif type == "drag":
                path = kwargs.get("path")
                if not path or not isinstance(path, list) or len(path) < 2:
                    raise ToolError("path is required for drag action and must contain at least 2 points")
                return await self.handle_drag(path)
            elif type == "screenshot":
                return await self.screenshot()
            elif type == "wait":
                duration = kwargs.get("duration", 1.0)
                await asyncio.sleep(duration)
                return await self.screenshot()
            else:
                raise ToolError(f"Unsupported action: {type}")

        except Exception as e:
            self.logger.error(f"Error in ComputerTool.__call__: {str(e)}")
            raise ToolError(f"Failed to execute {type}: {str(e)}")

    async def handle_click(self, button: str, x: int, y: int) -> ToolResult:
        """Handle different click actions."""
        try:
            # Perform requested click action
            if button == "left":
                await self.computer.interface.left_click(x, y)
            elif button == "right":
                await self.computer.interface.right_click(x, y)
            elif button == "double":
                await self.computer.interface.double_click(x, y)

            # Wait for UI to update
            await asyncio.sleep(0.5)

            # Take screenshot after action
            screenshot = await self.computer.interface.screenshot()
            base64_screenshot = base64.b64encode(screenshot).decode("utf-8")

            return ToolResult(
                output=f"Performed {button} click at ({x}, {y})",
                base64_image=base64_screenshot,
            )
        except Exception as e:
            self.logger.error(f"Error in handle_click: {str(e)}")
            raise ToolError(f"Failed to perform {button} click at ({x}, {y}): {str(e)}")

    async def handle_typing(self, text: str) -> ToolResult:
        """Handle typing text with a small delay between characters."""
        try:
            # Type the text with a small delay
            await self.computer.interface.type_text(text)

            await asyncio.sleep(0.3)

            # Take screenshot after typing
            screenshot = await self.computer.interface.screenshot()
            base64_screenshot = base64.b64encode(screenshot).decode("utf-8")

            return ToolResult(output=f"Typed: {text}", base64_image=base64_screenshot)
        except Exception as e:
            self.logger.error(f"Error in handle_typing: {str(e)}")
            raise ToolError(f"Failed to type '{text}': {str(e)}")

    async def handle_key(self, key: Union[str, List[str]]) -> ToolResult:
        """Handle key press, supporting both single keys and combinations.

        Args:
            key: Either a string (e.g. "ctrl+c") or a list of keys (e.g. ["ctrl", "c"])
        """
        try:
            # Check if key is already a list
            if isinstance(key, list):
                keys = [k.strip().lower() for k in key]
            else:
                # Split key string into list if it's a combination (e.g. "ctrl+c")
                keys = [k.strip().lower() for k in key.split("+")]

            # Map each key
            mapped_keys = [KEY_MAPPING.get(k, k) for k in keys]

            if len(mapped_keys) > 1:
                # For key combinations (like Ctrl+C)
                await self.computer.interface.hotkey(*mapped_keys)
            else:
                # Single key press
                await self.computer.interface.press_key(mapped_keys[0])

            # Wait briefly
            await asyncio.sleep(0.3)

            # Take screenshot after action
            screenshot = await self.computer.interface.screenshot()
            base64_screenshot = base64.b64encode(screenshot).decode("utf-8")

            return ToolResult(output=f"Pressed key: {key}", base64_image=base64_screenshot)
        except Exception as e:
            self.logger.error(f"Error in handle_key: {str(e)}")
            raise ToolError(f"Failed to press key '{key}': {str(e)}")

    async def handle_mouse_move(self, x: int, y: int) -> ToolResult:
        """Handle mouse movement."""
        try:
            # Move cursor to position
            await self.computer.interface.move_cursor(x, y)

            # Wait briefly
            await asyncio.sleep(0.2)

            # Take screenshot after action
            screenshot = await self.computer.interface.screenshot()
            base64_screenshot = base64.b64encode(screenshot).decode("utf-8")

            return ToolResult(output=f"Moved cursor to ({x}, {y})", base64_image=base64_screenshot)
        except Exception as e:
            self.logger.error(f"Error in handle_mouse_move: {str(e)}")
            raise ToolError(f"Failed to move cursor to ({x}, {y}): {str(e)}")

    async def handle_scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> ToolResult:
        """Handle scrolling."""
        try:
            # Move cursor to position first
            await self.computer.interface.move_cursor(x, y)

            # Scroll based on direction
            if scroll_y > 0:
                await self.computer.interface.scroll_down(abs(scroll_y))
            elif scroll_y < 0:
                await self.computer.interface.scroll_up(abs(scroll_y))

            # Wait for UI to update
            await asyncio.sleep(0.5)

            # Take screenshot after action
            screenshot = await self.computer.interface.screenshot()
            base64_screenshot = base64.b64encode(screenshot).decode("utf-8")

            return ToolResult(
                output=f"Scrolled at ({x}, {y}) with delta ({scroll_x}, {scroll_y})",
                base64_image=base64_screenshot,
            )
        except Exception as e:
            self.logger.error(f"Error in handle_scroll: {str(e)}")
            raise ToolError(f"Failed to scroll at ({x}, {y}): {str(e)}")

    async def handle_drag(self, path: List[Dict[str, int]]) -> ToolResult:
        """Handle mouse drag operation using a path of coordinates.

        Args:
            path: List of coordinate points {"x": int, "y": int} defining the drag path

        Returns:
            ToolResult with the operation result and screenshot
        """
        try:
            # Convert from [{"x": x, "y": y}, ...] format to [(x, y), ...] format
            points = [(p["x"], p["y"]) for p in path]
            
            # Perform drag action
            if len(points) == 2:
                await self.computer.interface.move_cursor(points[0][0], points[0][1])
                await self.computer.interface.drag_to(points[1][0], points[1][1])
            else:
                await self.computer.interface.drag(points, button="left")
            
            # Wait for UI to update
            await asyncio.sleep(0.5)
            
            # Take screenshot after action
            screenshot = await self.computer.interface.screenshot()
            base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
            
            return ToolResult(
                output=f"Dragged from ({path[0]['x']}, {path[0]['y']}) to ({path[-1]['x']}, {path[-1]['y']})",
                base64_image=base64_screenshot,
            )
        except Exception as e:
            self.logger.error(f"Error in handle_drag: {str(e)}")
            raise ToolError(f"Failed to perform drag operation: {str(e)}")

    async def screenshot(self) -> ToolResult:
        """Take a screenshot."""
        try:
            # Take screenshot
            screenshot = await self.computer.interface.screenshot()
            base64_screenshot = base64.b64encode(screenshot).decode("utf-8")

            return ToolResult(output="Screenshot taken", base64_image=base64_screenshot)
        except Exception as e:
            self.logger.error(f"Error in screenshot: {str(e)}")
            raise ToolError(f"Failed to take screenshot: {str(e)}")
