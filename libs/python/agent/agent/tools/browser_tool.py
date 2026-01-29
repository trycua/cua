"""
Browser Tool for agent interactions.
Allows agents to control a browser programmatically via Playwright.
Implements the computer_use action interface for comprehensive browser control.
"""

import asyncio
import logging
from typing import TYPE_CHECKING, Optional, Union

from .base import BaseComputerTool, register_tool

if TYPE_CHECKING:
    from computer.interface import GenericComputerInterface

logger = logging.getLogger(__name__)


@register_tool("computer_use")
class BrowserTool(BaseComputerTool):
    """
    Browser tool that uses the computer SDK's interface to control a browser.
    Implements a comprehensive computer_use action interface for browser control.
    """

    def __init__(self, interface: "GenericComputerInterface", cfg: Optional[dict] = None):
        """
        Initialize the BrowserTool.

        Args:
            interface: A GenericComputerInterface instance that provides playwright_exec
            cfg: Optional configuration dictionary
        """
        self.interface = interface
        self._facts = []  # Store memorized facts
        self._automation = None  # Cached automation interface

        # Get initial screenshot to determine dimensions
        self.viewport_width = None
        self.viewport_height = None
        self.resized_width = None
        self.resized_height = None

        # Try to initialize dimensions synchronously
        try:
            import asyncio

            loop = asyncio.get_event_loop()
            if loop.is_running():
                # If we're in an async context, dimensions will be lazy-loaded
                pass
            else:
                loop.run_until_complete(self._initialize_dimensions())
        except Exception:
            # Dimensions will be lazy-loaded on first use
            pass

        super().__init__(cfg)

    @property
    def automation(self):
        """
        Get the automation interface for keyboard/mouse actions.

        Handles both interface structures:
        - Nested: interface.interface (wrapper with .interface property)
        - Direct: interface itself IS the automation handler
        """
        if self._automation is not None:
            return self._automation

        # Try nested structure first (interface.interface)
        if hasattr(self.interface, "interface") and self.interface.interface is not None:
            self._automation = self.interface.interface
        else:
            # Direct structure - interface IS the automation handler
            self._automation = self.interface

        return self._automation

    async def _initialize_dimensions(self):
        """Initialize viewport and resized dimensions from screenshot."""
        try:
            import base64
            import io

            from PIL import Image
            from qwen_vl_utils import smart_resize

            # Take a screenshot to get actual dimensions
            screenshot_b64 = await self.screenshot()
            img_bytes = base64.b64decode(screenshot_b64)
            im = Image.open(io.BytesIO(img_bytes))

            # Store actual viewport size
            self.viewport_width = im.width
            self.viewport_height = im.height

            # Calculate resized dimensions using smart_resize with factor=28
            MIN_PIXELS = 3136
            MAX_PIXELS = 12845056
            rh, rw = smart_resize(
                im.height, im.width, factor=28, min_pixels=MIN_PIXELS, max_pixels=MAX_PIXELS
            )
            self.resized_width = rw
            self.resized_height = rh

        except Exception as e:
            # Fall back to defaults if initialization fails
            logger.warning(f"Failed to initialize dimensions: {e}")
            self.viewport_width = 1024
            self.viewport_height = 768
            self.resized_width = 1024
            self.resized_height = 768

    async def _proc_coords(self, x: float, y: float) -> tuple:
        """
        Process coordinates by converting from resized space to viewport space.

        Args:
            x: X coordinate in resized space (0 to resized_width)
            y: Y coordinate in resized space (0 to resized_height)

        Returns:
            Tuple of (viewport_x, viewport_y) in actual viewport pixels
        """
        # Ensure dimensions are initialized
        if self.resized_width is None or self.resized_height is None:
            await self._initialize_dimensions()

        # Convert from resized space to viewport space
        # Normalize by resized dimensions, then scale to viewport dimensions
        viewport_x = (x / self.resized_width) * self.viewport_width
        viewport_y = (y / self.resized_height) * self.viewport_height

        return int(round(viewport_x)), int(round(viewport_y))

    @property
    def description(self) -> str:
        # Use resized dimensions if available, otherwise use defaults
        width = self.resized_width if self.resized_width is not None else 1024
        height = self.resized_height if self.resized_height is not None else 768

        return f"Use a mouse and keyboard to interact with a computer, and take screenshots.\
* This is an interface to a desktop GUI. You do not have access to a terminal or applications menu. You must click on desktop icons to start applications.\
* Some applications may take time to start or process actions, so you may need to wait and take successive screenshots to see the results of your actions. E.g. if you click on Firefox and a window doesn't open, try wait and taking another screenshot.\
* The screen's resolution is {width}x{height}.\
* Whenever you intend to move the cursor to click on an element like an icon, you should consult a screenshot to determine the coordinates of the element before moving the cursor.\
* If you tried clicking on a program or link but it failed to load, even after waiting, try adjusting your cursor position so that the tip of the cursor visually falls on the element that you want to click.\
* Make sure to click any buttons, links, icons, etc with the cursor tip in the center of the element. Don't click boxes on their edges unless asked.\
* When a separate scrollable container prominently overlays the webpage, if you want to scroll within it, you typically need to mouse_move() over it first and then scroll().\
* If a popup window appears that you want to close, if left_click() on the 'X' or close button doesn't work, try key(keys=['Escape']) to close it.\
* On some search bars, when you type(), you may need to press_enter=False and instead separately call left_click() on the search button to submit the search query. This is especially true of search bars that have auto-suggest popups for e.g. locations\
* For calendar widgets, you usually need to left_click() on arrows to move between months and left_click() on dates to select them; type() is not typically used to input dates there.".strip()

    @property
    def parameters(self) -> dict:
        return {
            "type": "object",
            "properties": {
                "action": {
                    "description": """The action to perform. The available actions are:
* key: Performs key down presses on the arguments passed in order, then performs key releases in reverse order. Includes 'Enter', 'Alt', 'Shift', 'Tab', 'Control', 'Backspace', 'Delete', 'Escape', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageDown', 'PageUp', 'Shift', etc.
* type: Type a string of text on the keyboard.
* mouse_move: Move the cursor to a specified (x, y) pixel coordinate on the screen.
* left_click: Click the left mouse button.
* scroll: Performs a scroll of the mouse scroll wheel.
* visit_url: Visit a specified URL.
* web_search: Perform a web search with a specified query.
* history_back: Go back to the previous page in the browser history.
* pause_and_memorize_fact: Pause and memorize a fact for future reference.
* wait: Wait specified seconds for the change to happen.
* terminate: Terminate the current task and report its completion status.""",
                    "enum": [
                        "key",
                        "type",
                        "mouse_move",
                        "left_click",
                        "scroll",
                        "visit_url",
                        "web_search",
                        "history_back",
                        "pause_and_memorize_fact",
                        "wait",
                        "terminate",
                    ],
                    "type": "string",
                },
                "keys": {"description": "Required only by action=key.", "type": "array"},
                "text": {"description": "Required only by action=type.", "type": "string"},
                "coordinate": {
                    "description": "(x, y) coordinates for mouse actions. Required only by action=left_click, action=mouse_move, and action=type.",
                    "type": "array",
                },
                "pixels": {
                    "description": "Amount of scrolling. Positive = up, Negative = down. Required only by action=scroll.",
                    "type": "number",
                },
                "url": {
                    "description": "The URL to visit. Required only by action=visit_url.",
                    "type": "string",
                },
                "query": {
                    "description": "The query to search for. Required only by action=web_search.",
                    "type": "string",
                },
                "fact": {
                    "description": "The fact to remember for the future. Required only by action=pause_and_memorize_fact.",
                    "type": "string",
                },
                "time": {
                    "description": "Seconds to wait. Required only by action=wait.",
                    "type": "number",
                },
                "status": {
                    "description": "Status of the task. Required only by action=terminate.",
                    "type": "string",
                    "enum": ["success", "failure"],
                },
            },
            "required": ["action"],
        }

    def call(self, params: Union[str, dict], **kwargs) -> Union[str, dict]:
        """
        Execute a browser action.

        Args:
            params: Action parameters (JSON string or dict)
            **kwargs: Additional keyword arguments

        Returns:
            Result of the action execution
        """
        # Verify and parse parameters
        params_dict = self._verify_json_format_args(params)
        action = params_dict.get("action")

        if not action:
            return {"success": False, "error": "action parameter is required"}

        # Execute action synchronously by running async method in event loop
        try:
            loop = asyncio.get_event_loop()
            if loop.is_running():
                # If we're already in an async context, we can't use run_until_complete
                # Create a task and wait for it
                import concurrent.futures

                with concurrent.futures.ThreadPoolExecutor() as executor:
                    future = executor.submit(asyncio.run, self._execute_action(action, params_dict))
                    result = future.result()
            else:
                result = loop.run_until_complete(self._execute_action(action, params_dict))
            return result
        except Exception as e:
            logger.error(f"Error executing action {action}: {e}")
            return {"success": False, "error": str(e)}

    async def _execute_action(self, action: str, params: dict) -> dict:
        """Execute the specific action asynchronously."""
        try:
            if action == "key":
                return await self._action_key(params)
            elif action == "type":
                return await self._action_type(params)
            elif action == "mouse_move":
                return await self._action_mouse_move(params)
            elif action == "left_click":
                return await self._action_left_click(params)
            elif action == "scroll":
                return await self._action_scroll(params)
            elif action == "visit_url":
                return await self._action_visit_url(params)
            elif action == "web_search":
                return await self._action_web_search(params)
            elif action == "history_back":
                return await self._action_history_back(params)
            elif action == "pause_and_memorize_fact":
                return await self._action_pause_and_memorize_fact(params)
            elif action == "wait":
                return await self._action_wait(params)
            elif action == "terminate":
                return await self._action_terminate(params)
            else:
                return {"success": False, "error": f"Unknown action: {action}"}
        except Exception as e:
            logger.error(f"Error in action {action}: {e}")
            return {"success": False, "error": str(e)}

    async def _action_key(self, params: dict) -> dict:
        """Press keys in sequence."""
        keys = params.get("keys", [])
        if not keys:
            return {"success": False, "error": "keys parameter is required"}

        # Convert keys to proper format and press via hotkey
        try:
            await self.automation.hotkey(*keys)
            return {"success": True, "message": f"Pressed keys: {keys}"}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def _action_type(self, params: dict) -> dict:
        """Type text."""
        text = params.get("text")
        if not text:
            return {"success": False, "error": "text parameter is required"}

        # If coordinate is provided, click there first
        coordinate = params.get("coordinate")
        if coordinate and len(coordinate) == 2:
            await self.interface.playwright_exec("click", {"x": coordinate[0], "y": coordinate[1]})

        result = await self.interface.playwright_exec("type", {"text": text})
        return result

    async def _action_mouse_move(self, params: dict) -> dict:
        """Move mouse to coordinates."""
        coordinate = params.get("coordinate")
        if not coordinate or len(coordinate) != 2:
            return {"success": False, "error": "coordinate parameter [x, y] is required"}

        await self.automation.move_cursor(coordinate[0], coordinate[1])
        return {"success": True, "message": f"Moved cursor to {coordinate}"}

    async def _action_left_click(self, params: dict) -> dict:
        """Click at coordinates."""
        coordinate = params.get("coordinate")
        if not coordinate or len(coordinate) != 2:
            return {"success": False, "error": "coordinate parameter [x, y] is required"}

        result = await self.interface.playwright_exec(
            "click", {"x": coordinate[0], "y": coordinate[1]}
        )
        return result

    async def _action_scroll(self, params: dict) -> dict:
        """Scroll the page."""
        pixels = params.get("pixels")
        # Handle None explicitly - default to 0 means "no scroll requested"
        if pixels is None or pixels == 0:
            return {"success": False, "error": "pixels parameter is required"}

        # Positive = up (negative delta_y), Negative = down (positive delta_y)
        result = await self.interface.playwright_exec("scroll", {"delta_x": 0, "delta_y": -pixels})
        return result

    async def _action_visit_url(self, params: dict) -> dict:
        """Visit a URL."""
        url = params.get("url")
        if not url:
            return {"success": False, "error": "url parameter is required"}

        result = await self.interface.playwright_exec("visit_url", {"url": url})
        return result

    async def _action_web_search(self, params: dict) -> dict:
        """Perform web search."""
        query = params.get("query")
        if not query:
            return {"success": False, "error": "query parameter is required"}

        result = await self.interface.playwright_exec("web_search", {"query": query})
        return result

    async def _action_history_back(self, params: dict) -> dict:
        """Go back in browser history."""
        # Press Alt+Left arrow key combination
        try:
            await self.automation.hotkey("Alt", "ArrowLeft")
            return {"success": True, "message": "Navigated back in history"}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def _action_pause_and_memorize_fact(self, params: dict) -> dict:
        """Memorize a fact."""
        fact = params.get("fact")
        if not fact:
            return {"success": False, "error": "fact parameter is required"}

        self._facts.append(fact)
        return {
            "success": True,
            "message": f"Memorized fact: {fact}",
            "total_facts": len(self._facts),
        }

    async def _action_wait(self, params: dict) -> dict:
        """Wait for specified seconds."""
        time = params.get("time")
        # Handle None or missing time - default to 3 seconds (matches FARA behavior)
        if time is None:
            time = 3
        if time <= 0:
            return {"success": False, "error": "time parameter must be positive"}

        await asyncio.sleep(time)
        return {"success": True, "message": f"Waited {time} seconds"}

    async def _action_terminate(self, params: dict) -> dict:
        """Terminate and report status."""
        status = params.get("status")
        # Handle None or missing status - default to "success"
        if status is None:
            status = "success"
        message = f"Task terminated with status: {status}"

        if self._facts:
            message += f"\nMemorized facts: {self._facts}"

        return {"success": True, "status": status, "message": message, "terminated": True}

    # Legacy methods for backward compatibility
    async def visit_url(self, url: str) -> dict:
        """Navigate to a URL."""
        return await self._action_visit_url({"url": url})

    async def click(self, x: int = None, y: int = None, button: str = "left", **kwargs) -> dict:
        """Click at coordinates. Supports both positional (x, y) and kwargs (button, x, y).

        This is compatible with the normalized format from OperatorNormalizerCallback
        which transforms actions like {"type": "left_click", "coordinate": [x, y]}
        into {"type": "click", "button": "left", "x": x, "y": y}.
        """
        if x is None or y is None:
            return {"success": False, "error": "x and y coordinates are required"}
        if button == "right":
            return await self.interface.playwright_exec(
                "click", {"x": x, "y": y, "button": "right"}
            )
        elif button == "middle" or button == "wheel":
            return await self.interface.playwright_exec(
                "click", {"x": x, "y": y, "button": "middle"}
            )
        else:
            # Default to left click
            return await self._action_left_click({"coordinate": [x, y]})

    async def type(self, text: str) -> dict:
        """Type text into the focused element."""
        return await self._action_type({"text": text})

    async def scroll(
        self,
        delta_x: int = None,
        delta_y: int = None,
        scroll_x: int = None,
        scroll_y: int = None,
        x: int = None,
        y: int = None,
        pixels: int = None,
        coordinate=None,
        **kwargs,
    ) -> dict:
        """Scroll the page. Supports multiple formats:
        - Legacy: scroll(delta_x, delta_y)
        - Normalized: scroll(scroll_x=0, scroll_y=100, x=500, y=300)
        - FARA: scroll(pixels=100, coordinate=[500, 300])
        """
        # Determine scroll amounts from various input formats
        dx = scroll_x or delta_x or 0
        dy = scroll_y or delta_y or (-(pixels or 0))  # pixels: positive=up, negative=down

        result = await self.interface.playwright_exec("scroll", {"delta_x": dx, "delta_y": dy})
        return result

    async def web_search(self, query: str) -> dict:
        """Navigate to a Google search for the query."""
        return await self._action_web_search({"query": query})

    async def screenshot(self) -> str:
        """Take a screenshot of the current browser page."""
        result = await self.interface.playwright_exec("screenshot", {})
        if result.get("success") and result.get("screenshot"):
            screenshot_b64 = result["screenshot"]
            return screenshot_b64
        else:
            error = result.get("error", "Unknown error")
            raise RuntimeError(f"Failed to take screenshot: {error}")

    async def get_current_url(self) -> str:
        """Get the current URL of the browser page."""
        result = await self.interface.playwright_exec("get_current_url", {})
        if result.get("success") and result.get("url"):
            return result["url"]
        else:
            error = result.get("error", "Unknown error")
            raise RuntimeError(f"Failed to get current URL: {error}")

    # FARA-compatible action methods
    # These methods accept parameters in the format that FARA model outputs
    # and agent.py passes via **action_args

    async def left_click(self, coordinate=None, x: int = None, y: int = None, **kwargs) -> dict:
        """Left click at coordinates. Supports coordinate array or x/y kwargs."""
        # Accept either coordinate array or x/y kwargs
        if coordinate and len(coordinate) >= 2:
            x, y = coordinate[0], coordinate[1]
        if x is None or y is None:
            return {"success": False, "error": "coordinate parameter [x, y] or x/y kwargs required"}
        return await self._action_left_click({"coordinate": [x, y]})

    async def right_click(self, coordinate=None, x: int = None, y: int = None, **kwargs) -> dict:
        """Right click at coordinates. Supports coordinate array or x/y kwargs."""
        # Accept either coordinate array or x/y kwargs
        if coordinate and len(coordinate) >= 2:
            x, y = coordinate[0], coordinate[1]
        if x is None or y is None:
            return {"success": False, "error": "coordinate parameter [x, y] or x/y kwargs required"}
        result = await self.interface.playwright_exec("click", {"x": x, "y": y, "button": "right"})
        return result

    async def middle_click(self, coordinate=None, x: int = None, y: int = None, **kwargs) -> dict:
        """Middle click at coordinates. Supports coordinate array or x/y kwargs."""
        # Accept either coordinate array or x/y kwargs
        if coordinate and len(coordinate) >= 2:
            x, y = coordinate[0], coordinate[1]
        if x is None or y is None:
            return {"success": False, "error": "coordinate parameter [x, y] or x/y kwargs required"}
        result = await self.interface.playwright_exec("click", {"x": x, "y": y, "button": "middle"})
        return result

    async def double_click(self, coordinate=None, x: int = None, y: int = None, **kwargs) -> dict:
        """Double click at coordinates. Supports coordinate array or x/y kwargs."""
        # Accept either coordinate array or x/y kwargs
        if coordinate and len(coordinate) >= 2:
            x, y = coordinate[0], coordinate[1]
        if x is None or y is None:
            return {"success": False, "error": "coordinate parameter [x, y] or x/y kwargs required"}
        result = await self.interface.playwright_exec("dblclick", {"x": x, "y": y})
        return result

    async def triple_click(
        self, coordinate=None, x: int = None, y: int = None, button: str = None, **kwargs
    ) -> dict:
        """Triple click at coordinates. Supports coordinate array or x/y kwargs."""
        # Accept either coordinate array or x/y kwargs
        if coordinate and len(coordinate) >= 2:
            x, y = coordinate[0], coordinate[1]
        if x is None or y is None:
            return {"success": False, "error": "coordinate parameter [x, y] or x/y kwargs required"}
        # Triple click is approximated as double click
        return await self.double_click(x=x, y=y)

    async def mouse_move(self, coordinate=None, x: int = None, y: int = None, **kwargs) -> dict:
        """Move mouse to coordinates. Supports coordinate array or x/y kwargs."""
        # Accept either coordinate array or x/y kwargs
        if coordinate and len(coordinate) >= 2:
            x, y = coordinate[0], coordinate[1]
        if x is None or y is None:
            return {"success": False, "error": "coordinate parameter [x, y] or x/y kwargs required"}
        return await self._action_mouse_move({"coordinate": [x, y]})

    async def move(self, x: int = None, y: int = None, **kwargs) -> dict:
        """Move mouse to coordinates. Alias for mouse_move with x/y kwargs."""
        return await self.mouse_move(x=x, y=y)

    async def left_click_drag(
        self, coordinate=None, start_coordinate=None, end_coordinate=None, **kwargs
    ) -> dict:
        """Drag from start to end coordinates. FARA-compatible."""
        if start_coordinate and end_coordinate:
            # Use start/end coordinates if provided
            await self.automation.move_cursor(start_coordinate[0], start_coordinate[1])
            await self.automation.mouse_down(start_coordinate[0], start_coordinate[1])
            await self.automation.move_cursor(end_coordinate[0], end_coordinate[1])
            await self.automation.mouse_up(end_coordinate[0], end_coordinate[1])
            return {
                "success": True,
                "message": f"Dragged from {start_coordinate} to {end_coordinate}",
            }
        elif coordinate:
            # Just move to coordinate
            await self.automation.move_cursor(coordinate[0], coordinate[1])
            return {"success": True, "message": f"Moved to {coordinate}"}
        return {
            "success": False,
            "error": "start_coordinate and end_coordinate or coordinate required",
        }

    async def key(self, keys=None, **kwargs) -> dict:
        """Press keys. FARA-compatible."""
        return await self._action_key({"keys": keys})

    async def keypress(self, keys=None, **kwargs) -> dict:
        """Press keys. Alias for key() - used by OperatorNormalizerCallback."""
        return await self._action_key({"keys": keys})

    async def hscroll(self, pixels=None, coordinate=None, **kwargs) -> dict:
        """Horizontal scroll. FARA-compatible."""
        if pixels is None:
            return {"success": False, "error": "pixels parameter is required"}
        result = await self.interface.playwright_exec("scroll", {"delta_x": pixels, "delta_y": 0})
        return result

    async def wait(self, time=None, **kwargs) -> dict:
        """Wait for specified seconds. FARA-compatible."""
        return await self._action_wait({"time": time})

    async def history_back(self, **kwargs) -> dict:
        """Go back in browser history. FARA-compatible."""
        return await self._action_history_back({})

    async def terminate(self, status=None, **kwargs) -> dict:
        """Terminate and report status. FARA-compatible."""
        return await self._action_terminate({"status": status or "success"})
