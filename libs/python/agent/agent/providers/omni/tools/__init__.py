"""Omni provider tools - compatible with multiple LLM providers."""

from ....core.tools import BaseTool, CLIResult, ToolError, ToolFailure, ToolResult
from .base import BaseOmniTool
from .bash import BashTool
from .computer import ComputerTool
from .manager import ToolManager

# Re-export the tools with Omni-specific names for backward compatibility
OmniToolResult = ToolResult
OmniToolError = ToolError
OmniToolFailure = ToolFailure
OmniCLIResult = CLIResult

# We'll export specific tools once implemented
__all__ = [
    "BaseTool",
    "BaseOmniTool",
    "ToolResult",
    "ToolError",
    "ToolFailure",
    "CLIResult",
    "OmniToolResult",
    "OmniToolError",
    "OmniToolFailure",
    "OmniCLIResult",
    "ComputerTool",
    "BashTool",
    "ToolManager",
]
