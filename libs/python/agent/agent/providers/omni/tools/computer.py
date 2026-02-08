"""Computer tool for Omni provider."""

import json
import logging
from typing import Any, Dict

from computer import Computer

from ....core.tools import ToolError, ToolResult
from ..parser import ParseResult
from .base import BaseOmniTool

logger = logging.getLogger(__name__)


class ComputerTool(BaseOmniTool):
    """Tool for interacting with the computer UI."""

    name = "computer"
    description = "Interact with the computer's graphical user interface"

    def __init__(self, computer: Computer):
        """Initialize the computer tool.

        Args:
            computer: Computer instance
        """
        super().__init__()
        self.computer = computer
        # Default to standard screen dimensions (will be set more accurately during initialization)
        self.screen_dimensions = {"width": 1440, "height": 900}

    async def initialize_dimensions(self) -> None:
        """Initialize screen dimensions."""
        # For now, we'll use default values
        # In the future, we can implement proper screen dimension detection
        logger.info(f"Using default screen dimensions: {self.screen_dimensions}")

    def to_params(self) -> Dict[str, Any]:
        """Convert tool to API parameters.

        Returns:
            Dictionary with tool parameters
        """
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": [
                                "left_click",
                                "right_click",
                                "double_click",
                                "move_cursor",
                                "drag_to",
                                "type_text",
                                "press_key",
                                "hotkey",
                                "scroll_up",
                                "scroll_down",
                            ],
                            "description": "The action to perform",
                        },
                        "x": {
                            "type": "number",
                            "description": "X coordinate for click or cursor movement",
                        },
                        "y": {
                            "type": "number",
                            "description": "Y coordinate for click or cursor movement",
                        },
                        "box_id": {
                            "type": "integer",
                            "description": "ID of the UI element to interact with",
                        },
                        "text": {
                            "type": "string",
                            "description": "Text to type",
                        },
                        "key": {
                            "type": "string",
                            "description": "Key to press",
                        },
                        "keys": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Keys to press as hotkey combination",
                        },
                        "amount": {
                            "type": "integer",
                            "description": "Amount to scroll",
                        },
                        "duration": {
                            "type": "number",
                            "description": "Duration for drag operations",
                        },
                    },
                    "required": ["action"],
                },
            },
        }

    async def __call__(self, **kwargs) -> ToolResult:
        """Execute computer action.

        Args:
            **kwargs: Action parameters

        Returns:
            Tool execution result
        """
        try:
            action = kwargs.get("action", "").lower()
            if not action:
                return ToolResult(error="No action specified")

            # Execute the action on the computer
            method = getattr(self.computer.interface, action, None)
            if not method:
                return ToolResult(error=f"Unsupported action: {action}")

            # Prepare arguments based on action type
            args = {}
            if action in ["left_click", "right_click", "double_click", "move_cursor"]:
                x = kwargs.get("x")
                y = kwargs.get("y")
                if x is None or y is None:
                    box_id = kwargs.get("box_id")
                    if box_id is None:
                        return ToolResult(error="Box ID or coordinates required")
                    # Get coordinates from box_id implementation would be here
                    # For now, return error
                    return ToolResult(error="Box ID-based clicking not implemented yet")
                args["x"] = x
                args["y"] = y
            elif action == "drag_to":
                x = kwargs.get("x")
                y = kwargs.get("y")
                if x is None or y is None:
                    return ToolResult(error="Coordinates required for drag_to")
                args.update(
                    {
                        "x": x,
                        "y": y,
                        "button": kwargs.get("button", "left"),
                        "duration": float(kwargs.get("duration", 0.5)),
                    }
                )
            elif action == "type_text":
                text = kwargs.get("text")
                if not text:
                    return ToolResult(error="Text required for type_text")
                args["text"] = text
            elif action == "press_key":
                key = kwargs.get("key")
                if not key:
                    return ToolResult(error="Key required for press_key")
                args["key"] = key
            elif action == "hotkey":
                keys = kwargs.get("keys")
                if not keys:
                    return ToolResult(error="Keys required for hotkey")
                # Call with positional arguments instead of kwargs
                await method(*keys)
                return ToolResult(output=f"Hotkey executed: {'+'.join(keys)}")
            elif action in ["scroll_down", "scroll_up"]:
                args["clicks"] = int(kwargs.get("amount", 1))

            # Execute action with prepared arguments
            await method(**args)
            return ToolResult(output=f"Action {action} executed successfully")

        except Exception as e:
            logger.error(f"Error executing computer action: {str(e)}")
            return ToolResult(error=f"Error: {str(e)}")
