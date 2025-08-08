"""HUD Computer Handler for ComputerAgent integration."""

import base64
from io import BytesIO
from typing import Literal, Optional, Any, Dict, Callable
from PIL import Image

from agent.computers import AsyncComputerHandler


class HUDComputerHandler(AsyncComputerHandler):
    """Computer handler that interfaces with HUD environment."""
    
    def __init__(
        self,
        environment: Literal["windows", "mac", "linux", "browser"] = "linux",
        dimensions: tuple[int, int] = (1024, 768),
        screenshot_callback: Optional[Callable] = None,
        action_callback: Optional[Callable] = None,
    ):
        """
        Initialize HUD computer handler.
        
        Args:
            environment: The environment type for HUD
            dimensions: Screen dimensions as (width, height)
            screenshot_callback: Optional callback to get screenshots from HUD environment
            action_callback: Optional callback to execute actions in HUD environment
        """
        super().__init__()
        self._environment = environment
        self._dimensions = dimensions
        self._screenshot_callback = screenshot_callback
        self._action_callback = action_callback
        
        # Store the last screenshot for reuse
        self._last_screenshot: Optional[str] = None
        
    def set_screenshot_callback(self, callback: Callable) -> None:
        """Set the screenshot callback."""
        self._screenshot_callback = callback
        
    def set_action_callback(self, callback: Callable) -> None:
        """Set the action callback."""
        self._action_callback = callback
        
    def update_screenshot(self, screenshot: str) -> None:
        """Update the stored screenshot (base64 string)."""
        self._last_screenshot = screenshot

    async def get_environment(self) -> Literal["windows", "mac", "linux", "browser"]:
        """Get the current environment type."""
        return self._environment # type: ignore
    
    async def get_dimensions(self) -> tuple[int, int]:
        """Get screen dimensions as (width, height)."""
        return self._dimensions
    
    async def screenshot(self) -> str:
        """Take a screenshot and return as base64 string."""
        if self._screenshot_callback:
            screenshot = await self._screenshot_callback()
            if isinstance(screenshot, str):
                self._last_screenshot = screenshot
                return screenshot
            elif isinstance(screenshot, Image.Image):
                # Convert PIL Image to base64
                buffer = BytesIO()
                screenshot.save(buffer, format="PNG")
                screenshot_b64 = base64.b64encode(buffer.getvalue()).decode()
                self._last_screenshot = screenshot_b64
                return screenshot_b64
            elif isinstance(screenshot, bytes):
                screenshot_b64 = base64.b64encode(screenshot).decode()
                self._last_screenshot = screenshot_b64
                return screenshot_b64
        
        # Return last screenshot if available, otherwise create a blank one
        if self._last_screenshot:
            return self._last_screenshot
            
        # Create a blank screenshot as fallback
        blank_image = Image.new('RGB', self._dimensions, color='white')
        buffer = BytesIO()
        blank_image.save(buffer, format="PNG")
        screenshot_b64 = base64.b64encode(buffer.getvalue()).decode()
        self._last_screenshot = screenshot_b64
        return screenshot_b64
    
    async def click(self, x: int, y: int, button: str = "left") -> None:
        """Click at coordinates with specified button."""
        if self._action_callback:
            await self._action_callback({
                "type": "click",
                "x": x,
                "y": y,
                "button": button
            })
    
    async def double_click(self, x: int, y: int) -> None:
        """Double click at coordinates."""
        if self._action_callback:
            await self._action_callback({
                "type": "double_click",
                "x": x,
                "y": y
            })
    
    async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
        """Scroll at coordinates with specified scroll amounts."""
        if self._action_callback:
            await self._action_callback({
                "type": "scroll",
                "x": x,
                "y": y,
                "scroll_x": scroll_x,
                "scroll_y": scroll_y
            })
    
    async def type(self, text: str) -> None:
        """Type text."""
        if self._action_callback:
            await self._action_callback({
                "type": "type",
                "text": text
            })
    
    async def wait(self, ms: int = 1000) -> None:
        """Wait for specified milliseconds."""
        if self._action_callback:
            await self._action_callback({
                "type": "wait",
                "ms": ms
            })
    
    async def move(self, x: int, y: int) -> None:
        """Move cursor to coordinates."""
        if self._action_callback:
            await self._action_callback({
                "type": "move",
                "x": x,
                "y": y
            })
    
    async def keypress(self, keys: list[str] | str) -> None:
        """Press key combination."""
        if isinstance(keys, str):
            keys = [keys]
        if self._action_callback:
            await self._action_callback({
                "type": "keypress",
                "keys": keys
            })
    
    async def drag(self, path: list[dict[str, int]]) -> None:
        """Drag along a path of points."""
        if self._action_callback:
            await self._action_callback({
                "type": "drag",
                "path": path
            })

    async def left_mouse_down(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse down at coordinates."""
        if self._action_callback:
            await self._action_callback({
                "type": "left_mouse_down",
                "x": x,
                "y": y
            })
    
    async def left_mouse_up(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse up at coordinates."""
        if self._action_callback:
            await self._action_callback({
                "type": "left_mouse_up",
                "x": x,
                "y": y
            })
    
    async def get_current_url(self) -> str:
        """Get the current URL."""
        if self._action_callback:
            return await self._action_callback({
                "type": "get_current_url"
            })
        return ""