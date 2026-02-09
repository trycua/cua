"""Anthropic-specific tools for agent."""

from .base import (
    AnthropicCLIResult,
    AnthropicToolError,
    AnthropicToolFailure,
    AnthropicToolResult,
    BaseAnthropicTool,
    CLIResult,
    ToolError,
    ToolFailure,
    ToolResult,
)
from .bash import BashTool
from .computer import ComputerTool
from .edit import EditTool
from .manager import ToolManager

__all__ = [
    "BaseAnthropicTool",
    "ToolResult",
    "ToolError",
    "ToolFailure",
    "CLIResult",
    "AnthropicToolResult",
    "AnthropicToolError",
    "AnthropicToolFailure",
    "AnthropicCLIResult",
    "BashTool",
    "ComputerTool",
    "EditTool",
    "ToolManager",
]
