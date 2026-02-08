"""Tool manager for the OpenAI provider."""

import logging
from typing import Any, Awaitable, Callable, Dict, List, Optional, Union

from computer import Computer

from ....core.tools.base import ToolFailure, ToolResult
from ....core.tools.collection import ToolCollection
from ..types import ComputerAction, ResponseItemType
from .computer import ComputerTool

logger = logging.getLogger(__name__)


class ToolManager:
    """Manager for computer tools in the OpenAI agent."""

    def __init__(
        self,
        computer: Computer,
        acknowledge_safety_check_callback: Optional[Callable[[str], Awaitable[bool]]] = None,
    ):
        """Initialize the tool manager.

        Args:
            computer: Computer instance
            acknowledge_safety_check_callback: Optional callback for safety check acknowledgment
        """
        self.computer = computer
        self.acknowledge_safety_check_callback = acknowledge_safety_check_callback
        self._initialized = False
        self.computer_tool = ComputerTool(computer)
        self.tools = None
        logger.info("Initialized OpenAI ToolManager")

    async def initialize(self) -> None:
        """Initialize the tool manager."""
        if not self._initialized:
            logger.info("Initializing OpenAI ToolManager")

            # Initialize the computer tool
            await self.computer_tool.initialize_dimensions()

            # Initialize tool collection
            self.tools = ToolCollection(self.computer_tool)

            self._initialized = True
            logger.info("OpenAI ToolManager initialized")

    async def get_tools_definition(self) -> List[Dict[str, Any]]:
        """Get the tools definition for the OpenAI agent.

        Returns:
            Tools definition for the OpenAI agent
        """
        if not self.tools:
            raise RuntimeError("Tools not initialized. Call initialize() first.")

        # For the OpenAI Agent Response API, we use a special "computer-preview" tool
        # which provides the correct interface for computer control
        display_width, display_height = await self._get_computer_dimensions()

        # Get environment, using "mac" as default since we're on macOS
        environment = getattr(self.computer, "environment", "mac")

        # Ensure environment is one of the allowed values
        if environment not in ["windows", "mac", "linux", "browser"]:
            logger.warning(f"Invalid environment value: {environment}, using 'mac' instead")
            environment = "mac"

        return [
            {
                "type": "computer-preview",
                "display_width": display_width,
                "display_height": display_height,
                "environment": environment,
            }
        ]

    async def _get_computer_dimensions(self) -> tuple[int, int]:
        """Get the dimensions of the computer display.

        Returns:
            Tuple of (width, height)
        """
        # If computer tool is initialized, use its dimensions
        if self.computer_tool.width is not None and self.computer_tool.height is not None:
            return (self.computer_tool.width, self.computer_tool.height)

        # Try to get from computer.interface if available
        screen_size = await self.computer.interface.get_screen_size()
        return (int(screen_size["width"]), int(screen_size["height"]))

    async def execute_tool(self, name: str, tool_input: Dict[str, Any]) -> ToolResult:
        """Execute a tool with the given input.

        Args:
            name: Name of the tool to execute
            tool_input: Input parameters for the tool

        Returns:
            Result of the tool execution
        """
        if not self.tools:
            raise RuntimeError("Tools not initialized. Call initialize() first.")
        return await self.tools.run(name=name, tool_input=tool_input)
