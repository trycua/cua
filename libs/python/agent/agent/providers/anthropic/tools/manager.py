from typing import Any, Dict, List, cast

from anthropic.types.beta import BetaToolUnionParam
from computer.computer import Computer

from ....core.tools import BaseToolManager, ToolResult
from ....core.tools.collection import ToolCollection
from .bash import BashTool
from .computer import ComputerTool
from .edit import EditTool


class ToolManager(BaseToolManager):
    """Manages Anthropic-specific tool initialization and execution."""

    def __init__(self, computer: Computer):
        """Initialize the tool manager.

        Args:
            computer: Computer instance for computer-related tools
        """
        super().__init__(computer)
        # Initialize Anthropic-specific tools
        self.computer_tool = ComputerTool(self.computer)
        self.bash_tool = BashTool(self.computer)
        self.edit_tool = EditTool(self.computer)

    def _initialize_tools(self) -> ToolCollection:
        """Initialize all available tools."""
        return ToolCollection(self.computer_tool, self.bash_tool, self.edit_tool)

    async def _initialize_tools_specific(self) -> None:
        """Initialize Anthropic-specific tool requirements."""
        await self.computer_tool.initialize_dimensions()

    def get_tool_params(self) -> List[BetaToolUnionParam]:
        """Get tool parameters for Anthropic API calls."""
        if self.tools is None:
            raise RuntimeError("Tools not initialized. Call initialize() first.")
        return cast(List[BetaToolUnionParam], self.tools.to_params())

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
