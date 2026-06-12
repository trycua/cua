"""cua.tools — agent tool classes and registry.

Usage::

    from cua.tools import BrowserTool, BaseTool, register_tool
"""

from cua_agent.tools import (
    TOOL_REGISTRY,
    BaseComputerTool,
    BaseTool,
    BrowserTool,
    get_registered_tools,
    get_tool,
    register_tool,
)
from cua_agent.types import IllegalArgumentError, ToolError

__all__ = [
    "BaseTool",
    "BaseComputerTool",
    "register_tool",
    "get_registered_tools",
    "get_tool",
    "TOOL_REGISTRY",
    "BrowserTool",
    "ToolError",
    "IllegalArgumentError",
]
