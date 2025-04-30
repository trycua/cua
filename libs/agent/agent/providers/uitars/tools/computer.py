"""Computer tool for UI-TARS."""

import asyncio
import base64
import logging
import re
from typing import Any, Dict, List, Optional, Literal, Union

from computer import Computer
from ....core.tools.base import ToolResult, ToolFailure
from ....core.tools.computer import BaseComputerTool

logger = logging.getLogger(__name__)


class ComputerTool(BaseComputerTool):
    """
    A tool that allows the UI-TARS agent to interact with the screen, keyboard, and mouse.
    """

    name: str = "computer"
    width: Optional[int] = None
    height: Optional[int] = None
    computer: Computer
    
    def __init__(self, computer: Computer):
        """Initialize the computer tool.

        Args:
            computer: Computer instance
        """
        super().__init__(computer)
        self.computer = computer
        self.width = None
        self.height = None
        self.logger = logging.getLogger(__name__)
        
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
            "type": "computer",
            "display_width": self.width,
            "display_height": self.height,
        }

    async def initialize_dimensions(self) -> None:
        """Initialize screen dimensions from the computer interface."""
        try:
            display_size = await self.computer.interface.get_screen_size()
            self.width = display_size["width"]
            self.height = display_size["height"]
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
        action: str,
        **kwargs,
    ) -> ToolResult:
        """Execute a computer action.
        
        Args:
            action: The action to perform (based on UI-TARS action space)
            **kwargs: Additional parameters for the action
            
        Returns:
            ToolResult containing action output and possibly a base64 image
        """
        try:
            # Ensure dimensions are initialized
            if self.width is None or self.height is None:
                await self.initialize_dimensions()
                if self.width is None or self.height is None:
                    return ToolFailure(error="Failed to initialize screen dimensions")
            
            # Handle actions defined in UI-TARS action space (from prompts.py)
            # Handle standard click (left click)
            if action == "click":
                if "x" in kwargs and "y" in kwargs:
                    x, y = kwargs["x"], kwargs["y"]
                    await self.computer.interface.left_click(x, y)
                    
                    # Wait briefly for UI to update
                    await asyncio.sleep(0.5)
                    
                    # Take screenshot after action
                    screenshot = await self.computer.interface.screenshot()
                    base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                    
                    return ToolResult(
                        output=f"Clicked at ({x}, {y})",
                        base64_image=base64_screenshot,
                    )
                else:
                    return ToolFailure(error="Missing coordinates for click action")
            
            # Handle double click
            elif action == "left_double":
                if "x" in kwargs and "y" in kwargs:
                    x, y = kwargs["x"], kwargs["y"]
                    await self.computer.interface.double_click(x, y)
                    
                    # Wait briefly for UI to update
                    await asyncio.sleep(0.5)
                    
                    # Take screenshot after action
                    screenshot = await self.computer.interface.screenshot()
                    base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                    
                    return ToolResult(
                        output=f"Double-clicked at ({x}, {y})",
                        base64_image=base64_screenshot,
                    )
                else:
                    return ToolFailure(error="Missing coordinates for left_double action")
            
            # Handle right click
            elif action == "right_single":
                if "x" in kwargs and "y" in kwargs:
                    x, y = kwargs["x"], kwargs["y"]
                    await self.computer.interface.right_click(x, y)
                    
                    # Wait briefly for UI to update
                    await asyncio.sleep(0.5)
                    
                    # Take screenshot after action
                    screenshot = await self.computer.interface.screenshot()
                    base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                    
                    return ToolResult(
                        output=f"Right-clicked at ({x}, {y})",
                        base64_image=base64_screenshot,
                    )
                else:
                    return ToolFailure(error="Missing coordinates for right_single action")
            
            # Handle typing text
            elif action == "type_text":
                if "text" in kwargs:
                    text = kwargs["text"]
                    await self.computer.interface.type_text(text)
                    
                    # Wait for UI to update
                    await asyncio.sleep(0.3)
                    
                    # Take screenshot after action
                    screenshot = await self.computer.interface.screenshot()
                    base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                    
                    return ToolResult(
                        output=f"Typed: {text}",
                        base64_image=base64_screenshot,
                    )
                else:
                    return ToolFailure(error="Missing text for type action")
            
            # Handle hotkey
            elif action == "hotkey":
                if "keys" in kwargs:
                    keys = kwargs["keys"]
                    
                    if len(keys) > 1:
                        await self.computer.interface.hotkey(*keys)
                    else:
                        # Single key press
                        await self.computer.interface.press_key(keys[0])
                        
                    # Wait for UI to update
                    await asyncio.sleep(0.3)
                    
                    # Take screenshot after action
                    screenshot = await self.computer.interface.screenshot()
                    base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                    
                    return ToolResult(
                        output=f"Pressed hotkey: {', '.join(keys)}",
                        base64_image=base64_screenshot,
                    )
                else:
                    return ToolFailure(error="Missing keys for hotkey action")
            
            # Handle drag action
            elif action == "drag":
                if all(k in kwargs for k in ["start_x", "start_y", "end_x", "end_y"]):
                    start_x, start_y = kwargs["start_x"], kwargs["start_y"]
                    end_x, end_y = kwargs["end_x"], kwargs["end_y"]
                    
                    # Perform drag
                    await self.computer.interface.move_cursor(start_x, start_y)
                    await self.computer.interface.drag_to(end_x, end_y)
                    
                    # Wait for UI to update
                    await asyncio.sleep(0.5)
                    
                    # Take screenshot after action
                    screenshot = await self.computer.interface.screenshot()
                    base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                    
                    return ToolResult(
                        output=f"Dragged from ({start_x}, {start_y}) to ({end_x}, {end_y})",
                        base64_image=base64_screenshot,
                    )
                else:
                    return ToolFailure(error="Missing coordinates for drag action")
            
            # Handle scroll action
            elif action == "scroll":
                if all(k in kwargs for k in ["x", "y", "direction"]):
                    x, y = kwargs["x"], kwargs["y"]
                    direction = kwargs["direction"]
                    
                    # Move cursor to position
                    await self.computer.interface.move_cursor(x, y)
                    
                    # Scroll based on direction
                    if direction == "down":
                        await self.computer.interface.scroll_down(5)
                    elif direction == "up":
                        await self.computer.interface.scroll_up(5)
                    elif direction == "right":
                        pass # await self.computer.interface.scroll_right(5)
                    elif direction == "left":
                        pass # await self.computer.interface.scroll_left(5)
                    else:
                        return ToolFailure(error=f"Invalid scroll direction: {direction}")
                    
                    # Wait for UI to update
                    await asyncio.sleep(0.5)
                    
                    # Take screenshot after action
                    screenshot = await self.computer.interface.screenshot()
                    base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                    
                    return ToolResult(
                        output=f"Scrolled {direction} at ({x}, {y})",
                        base64_image=base64_screenshot,
                    )
                else:
                    return ToolFailure(error="Missing parameters for scroll action")
            
            # Handle wait action
            elif action == "wait":
                # Sleep for 5 seconds as specified in the action space
                await asyncio.sleep(5)
                
                # Take screenshot after waiting
                screenshot = await self.computer.interface.screenshot()
                base64_screenshot = base64.b64encode(screenshot).decode("utf-8")
                
                return ToolResult(
                    output="Waited for 5 seconds",
                    base64_image=base64_screenshot,
                )
            
            # Handle finished action (task completion)
            elif action == "finished":
                content = kwargs.get("content", "Task completed")
                return ToolResult(
                    output=f"Task finished: {content}",
                )
            
                return await self._handle_scroll(action)
            else:
                return ToolFailure(error=f"Unsupported action: {action}")
                
        except Exception as e:
            self.logger.error(f"Error in ComputerTool.__call__: {str(e)}")
            return ToolFailure(error=f"Failed to execute {action}: {str(e)}")
