"""Python SDK and binary wrapper for the cross-platform cua-driver MCP server.

The package bundles the native Rust binary and provides synchronous and async
SDK interfaces for its MCP stdio protocol.
"""

__version__ = "0.10.0"  # x-release-please-version

from ._generated import (
    CONTRACT_VERSION,
    MCP_PROTOCOL_VERSION,
    ClickArgs,
    DragArgs,
    EndSessionArgs,
    EscalateSessionArgs,
    GetCursorPositionArgs,
    GetDesktopStateArgs,
    GetScreenSizeArgs,
    GetSessionStateArgs,
    HotkeyArgs,
    MoveCursorArgs,
    PressKeyArgs,
    ScrollArgs,
    StartSessionArgs,
    TypeTextArgs,
)
from .driver import AsyncCuaDriver, CuaDriver
from .result import ImageContent, ToolResult
from .transport import (
    AsyncStdioMcpTransport,
    AsyncTransport,
    McpResponseError,
    StdioMcpTransport,
    Transport,
)
from .wrapper import get_binary_path, run_cua_driver

__all__ = [
    "AsyncCuaDriver",
    "AsyncStdioMcpTransport",
    "AsyncTransport",
    "ClickArgs",
    "CONTRACT_VERSION",
    "CuaDriver",
    "DragArgs",
    "EndSessionArgs",
    "EscalateSessionArgs",
    "GetCursorPositionArgs",
    "GetDesktopStateArgs",
    "GetScreenSizeArgs",
    "GetSessionStateArgs",
    "HotkeyArgs",
    "ImageContent",
    "MCP_PROTOCOL_VERSION",
    "McpResponseError",
    "MoveCursorArgs",
    "PressKeyArgs",
    "ScrollArgs",
    "StartSessionArgs",
    "StdioMcpTransport",
    "ToolResult",
    "Transport",
    "TypeTextArgs",
    "__version__",
    "get_binary_path",
    "run_cua_driver",
]
