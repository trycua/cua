"""Collection classes for managing multiple tools."""

from typing import Any, Dict, List, Type

from .base import (
    BaseTool,
    ToolError,
    ToolFailure,
    ToolResult,
)


class ToolCollection:
    """A collection of tools that can be used with any provider."""

    def __init__(self, *tools: BaseTool):
        self.tools = tools
        self.tool_map = {tool.name: tool for tool in tools}

    def to_params(self) -> List[Dict[str, Any]]:
        """Convert all tools to provider-specific parameters.

        Returns:
            List of dictionaries with tool parameters
        """
        return [tool.to_params() for tool in self.tools]

    async def run(self, *, name: str, tool_input: Dict[str, Any]) -> ToolResult:
        """Run a tool with the given input.

        Args:
            name: Name of the tool to run
            tool_input: Input parameters for the tool

        Returns:
            Result of the tool execution
        """
        tool = self.tool_map.get(name)
        if not tool:
            return ToolFailure(error=f"Tool {name} is invalid")
        try:
            return await tool(**tool_input)
        except ToolError as e:
            return ToolFailure(error=e.message)
        except Exception as e:
            return ToolFailure(error=f"Unexpected error in tool {name}: {str(e)}")
