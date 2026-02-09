"""Tool manager for the Omni provider."""

from typing import Any, Dict, List

from computer.computer import Computer

from ....core.tools import BaseToolManager, ToolResult
from ....core.tools.collection import ToolCollection
from ....core.types import LLMProvider
from .bash import BashTool
from .computer import ComputerTool


class ToolManager(BaseToolManager):
    """Manages Omni provider tool initialization and execution."""

    def __init__(self, computer: Computer, provider: LLMProvider):
        """Initialize the tool manager.

        Args:
            computer: Computer instance for computer-related tools
            provider: The LLM provider being used
        """
        super().__init__(computer)
        self.provider = provider
        # Initialize Omni-specific tools
        self.computer_tool = ComputerTool(self.computer)
        self.bash_tool = BashTool(self.computer)

    def _initialize_tools(self) -> ToolCollection:
        """Initialize all available tools."""
        return ToolCollection(self.computer_tool, self.bash_tool)

    async def _initialize_tools_specific(self) -> None:
        """Initialize Omni provider-specific tool requirements."""
        await self.computer_tool.initialize_dimensions()

    def get_tool_params(self) -> List[Dict[str, Any]]:
        """Get tool parameters for API calls.

        Returns:
            List of tool parameters for the current provider's API
        """
        if self.tools is None:
            raise RuntimeError("Tools not initialized. Call initialize() first.")

        return self.tools.to_params()

    async def execute_tool(self, name: str, tool_input: dict[str, Any]) -> ToolResult:
        """Execute a tool with the given input.

        Args:
            name: Name of the tool to execute
            tool_input: Input parameters for the tool

        Returns:
            Result of the tool execution
        """
        if self.tools is None:
            raise RuntimeError("Tools not initialized. Call initialize() first.")

        return await self.tools.run(name=name, tool_input=tool_input)
