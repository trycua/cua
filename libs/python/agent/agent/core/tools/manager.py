"""Tool manager for initializing and running tools."""

from abc import ABC, abstractmethod
from typing import Any, Dict, List

from computer.computer import Computer

from .base import BaseTool, ToolResult
from .collection import ToolCollection


class BaseToolManager(ABC):
    """Base class for tool managers across different providers."""

    def __init__(self, computer: Computer):
        """Initialize the tool manager.

        Args:
            computer: Computer instance for computer-related tools
        """
        self.computer = computer
        self.tools: ToolCollection | None = None

    @abstractmethod
    def _initialize_tools(self) -> ToolCollection:
        """Initialize all available tools."""
        ...

    async def initialize(self) -> None:
        """Initialize tool-specific requirements and create tool collection."""
        await self._initialize_tools_specific()
        self.tools = self._initialize_tools()

    @abstractmethod
    async def _initialize_tools_specific(self) -> None:
        """Initialize provider-specific tool requirements."""
        ...

    @abstractmethod
    def get_tool_params(self) -> List[Dict[str, Any]]:
        """Get tool parameters for API calls."""
        ...

    async def execute_tool(self, name: str, tool_input: Dict[str, Any]) -> ToolResult:
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
