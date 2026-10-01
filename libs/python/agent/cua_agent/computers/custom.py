"""
Custom computer handler implementation that accepts a dictionary of functions.
"""

import base64
import io
from typing import Any, Callable, Dict, List, Literal, Optional, Union

from PIL import Image

from .base import AsyncComputerHandler


class CustomComputerHandler(AsyncComputerHandler):
    """Computer handler that implements the Computer protocol using a dictionary of custom functions."""

    def __init__(self, functions: Dict[str, Callable]):
        """
        Initialize with a dictionary of functions.

        Args:
            functions: Dictionary where keys are method names and values are callable functions.
                      Only 'screenshot' is required, all others are optional.

        Raises:
            ValueError: If required 'screenshot' function is not provided.
        """
        if "screenshot" not in functions:
            raise ValueError("'screenshot' function is required in functions dictionary")

        self.functions = functions
        self._last_screenshot_size: Optional[tuple[int, int]] = None

    async def _call_function(self, func, *args, **kwargs):
        """
        Call a function, handling both async and sync functions.

        Args:
            func: The function to call
            *args: Positional arguments to pass to the function
            **kwargs: Keyword arguments to pass to the function

        Returns:
            The result of the function call
        """
        import asyncio
        import inspect

        if callable(func):
            if inspect.iscoroutinefunction(func):
                return await func(*args, **kwargs)
            else:
                return func(*args, **kwargs)
        else:
            return func

    async def _get_value(self, attribute: str):
        """
        Get value for an attribute, checking both 'get_{attribute}' and '{attribute}' keys.

        Args:
            attribute: The attribute name to look for

        Returns:
            The value from the functions dict, called if callable, returned directly if not
        """
        # Check for 'get_{attribute}' first
        get_key = f"get_{attribute}"
        if get_key in self.functions:
            return await self._call_function(self.functions[get_key])

        # Check for '{attribute}'
        if attribute in self.functions:
            return await self._call_function(self.functions[attribute])

        return None

    def _to_b64_str(self, img: Union[bytes, Image.Image, str]) -> str:
        """
        Convert image to base64 string.

        Args:
            img: Image as bytes, PIL Image, or base64 string

        Returns:
            str: Base64 encoded image string
        """
        if isinstance(img, str):
            # Already a base64 string
            return img
        elif isinstance(img, bytes):
            # Raw bytes
            return base64.b64encode(img).decode("utf-8")
        elif isinstance(img, Image.Image):
            # PIL Image
            buffer = io.BytesIO()
            img.save(buffer, format="PNG")
            return base64.b64encode(buffer.getvalue()).decode("utf-8")
        else:
            raise ValueError(f"Unsupported image type: {type(img)}")

    # ==== Computer-Use-Preview Action Space ====

    async def get_environment(self) -> Literal["windows", "mac", "linux", "browser"]:
        """Get the current environment type."""
        result = await self._get_value("environment")
        if result is None:
            return "linux"
        assert result in ["windows", "mac", "linux", "browser"]
        return result  # type: ignore

    async def get_dimensions(self) -> tuple[int, int]:
        """Get screen dimensions as (width, height)."""
        result = await self._get_value("dimensions")
        if result is not None:
            return result  # type: ignore

        # Fallback: use last screenshot size if available
        if not self._last_screenshot_size:
            await self.screenshot()
        assert self._last_screenshot_size is not None, "Failed to get screenshot size"

        return self._last_screenshot_size

    async def screenshot(self, text: Optional[str] = None) -> str:
        """Take a screenshot and return as base64 string.

        Args:
            text: Optional descriptive text (for compatibility with GPT-4o models, ignored)
        """
        result = await self._call_function(self.functions["screenshot"])
        b64_str = self._to_b64_str(result)  # type: ignore

        # Try to extract dimensions for fallback use
        try:
            if isinstance(result, Image.Image):
                self._last_screenshot_size = result.size
            elif isinstance(result, bytes):
                # Try to decode bytes to get dimensions
                img = Image.open(io.BytesIO(result))
                self._last_screenshot_size = img.size
        except Exception:
            # If we can't get dimensions, that's okay
            pass

        return b64_str

    async def click(self, x: int, y: int, button: str = "left") -> None:
        """Click at coordinates with specified button."""
        if "click" in self.functions:
            await self._call_function(self.functions["click"], x, y, button)
        # No-op if not implemented

    async def double_click(self, x: int, y: int) -> None:
        """Double click at coordinates."""
        if "double_click" in self.functions:
            await self._call_function(self.functions["double_click"], x, y)
        # No-op if not implemented

    async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
        """Scroll at coordinates with specified scroll amounts."""
        if "scroll" in self.functions:
            await self._call_function(self.functions["scroll"], x, y, scroll_x, scroll_y)
        # No-op if not implemented

    async def type(self, text: str) -> None:
        """Type text."""
        if "type" in self.functions:
            await self._call_function(self.functions["type"], text)
        # No-op if not implemented

    async def wait(self, ms: int = 1000) -> None:
        """Wait for specified milliseconds."""
        if "wait" in self.functions:
            await self._call_function(self.functions["wait"], ms)
        else:
            # Default implementation
            import asyncio

            await asyncio.sleep(ms / 1000.0)

    async def move(self, x: int, y: int) -> None:
        """Move cursor to coordinates."""
        if "move" in self.functions:
            await self._call_function(self.functions["move"], x, y)
        # No-op if not implemented

    async def keypress(self, keys: Union[List[str], str]) -> None:
        """Press key combination."""
        if "keypress" in self.functions:
            await self._call_function(self.functions["keypress"], keys)
        # No-op if not implemented

    async def drag(self, path: List[Dict[str, int]]) -> None:
        """Drag along specified path."""
        if "drag" in self.functions:
            await self._call_function(self.functions["drag"], path)
        # No-op if not implemented

    async def get_current_url(self) -> str:
        """Get current URL (for browser environments)."""
        if "get_current_url" in self.functions:
            return await self._get_value("current_url")  # type: ignore
        return ""  # Default fallback

    async def left_mouse_down(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse down at coordinates."""
        if "left_mouse_down" in self.functions:
            await self._call_function(self.functions["left_mouse_down"], x, y)
        # No-op if not implemented

    async def left_mouse_up(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse up at coordinates."""
        if "left_mouse_up" in self.functions:
            await self._call_function(self.functions["left_mouse_up"], x, y)
        # No-op if not implemented
