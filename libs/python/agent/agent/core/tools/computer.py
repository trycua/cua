"""Abstract base computer tool implementation."""

import asyncio
import base64
import io
import logging
from abc import abstractmethod
from typing import Any, Dict, Optional, Tuple

from computer.computer import Computer
from PIL import Image

from .base import BaseTool, ToolError, ToolResult


class BaseComputerTool(BaseTool):
    """Base class for computer interaction tools across different providers."""

    name = "computer"
    logger = logging.getLogger(__name__)

    width: Optional[int] = None
    height: Optional[int] = None
    display_num: Optional[int] = None
    computer: Computer

    _screenshot_delay = 1.0  # Default delay for most platforms
    _scaling_enabled = True

    def __init__(self, computer: Computer):
        """Initialize the ComputerTool.

        Args:
            computer: Computer instance for screen interactions
        """
        self.computer = computer

    async def initialize_dimensions(self):
        """Initialize screen dimensions from the computer interface."""
        display_size = await self.computer.interface.get_screen_size()
        self.width = display_size["width"]
        self.height = display_size["height"]
        self.logger.info(f"Initialized screen dimensions to {self.width}x{self.height}")

    @property
    def options(self) -> Dict[str, Any]:
        """Get the options for the tool.

        Returns:
            Dictionary with tool options
        """
        if self.width is None or self.height is None:
            raise RuntimeError(
                "Screen dimensions not initialized. Call initialize_dimensions() first."
            )
        return {
            "display_width_px": self.width,
            "display_height_px": self.height,
            "display_number": self.display_num,
        }

    async def resize_screenshot_if_needed(self, screenshot: bytes) -> bytes:
        """Resize a screenshot to match the expected dimensions.

        Args:
            screenshot: Raw screenshot data

        Returns:
            Resized screenshot data
        """
        if self.width is None or self.height is None:
            raise ToolError("Screen dimensions not initialized")

        try:
            img = Image.open(io.BytesIO(screenshot))
            if img.mode in ("RGBA", "LA") or (img.mode == "P" and "transparency" in img.info):
                img = img.convert("RGB")

            # Resize if dimensions don't match
            if img.size != (self.width, self.height):
                self.logger.info(
                    f"Scaling image from {img.size} to {self.width}x{self.height} to match screen dimensions"
                )
                img = img.resize((self.width, self.height), Image.Resampling.LANCZOS)

                # Save back to bytes
                buffer = io.BytesIO()
                img.save(buffer, format="PNG")
                return buffer.getvalue()

            return screenshot
        except Exception as e:
            self.logger.error(f"Error during screenshot resizing: {str(e)}")
            raise ToolError(f"Failed to resize screenshot: {str(e)}")

    async def screenshot(self) -> ToolResult:
        """Take a screenshot and return it as a ToolResult with base64-encoded image.

        Returns:
            ToolResult with the screenshot
        """
        try:
            screenshot = await self.computer.interface.screenshot()
            screenshot = await self.resize_screenshot_if_needed(screenshot)
            return ToolResult(base64_image=base64.b64encode(screenshot).decode())
        except Exception as e:
            self.logger.error(f"Error taking screenshot: {str(e)}")
            return ToolResult(error=f"Failed to take screenshot: {str(e)}")

    @abstractmethod
    async def __call__(self, **kwargs) -> ToolResult:
        """Execute the tool with the provided arguments."""
        raise NotImplementedError
