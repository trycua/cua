"""Rust-backed SDK and binary wrapper for Cua Driver client applications.

Agents should configure the bundled ``cua-driver mcp`` executable directly
through their runtime's MCP client instead of importing a language MCP facade.
"""

__version__ = "0.10.0"  # x-release-please-version

from ._native import CuaDriver, DriverError, ImageContent, ToolResult
from ._native_contract import (
    CaptureScope,
    ClickButton,
    ClickInput,
    DesktopScope,
    DragInput,
    EndSessionInput,
    EndSessionOutput,
    EffectiveScope,
    EscalateSessionInput,
    EscalationReason,
    GetCursorPositionInput,
    GetDesktopStateInput,
    GetScreenSizeInput,
    GetSessionStateInput,
    HotkeyInput,
    MoveCursorInput,
    Platform,
    PressKeyInput,
    ScrollBy,
    ScrollDirection,
    ScrollInput,
    SessionStateOutput,
    StartSessionInput,
    StartSessionOutput,
    TypeTextInput,
)
from .wrapper import get_binary_path, run_cua_driver

__all__ = [
    "CaptureScope",
    "ClickButton",
    "ClickInput",
    "CuaDriver",
    "DesktopScope",
    "DragInput",
    "DriverError",
    "EffectiveScope",
    "EndSessionInput",
    "EndSessionOutput",
    "EscalateSessionInput",
    "EscalationReason",
    "GetCursorPositionInput",
    "GetDesktopStateInput",
    "GetScreenSizeInput",
    "GetSessionStateInput",
    "HotkeyInput",
    "ImageContent",
    "MoveCursorInput",
    "Platform",
    "PressKeyInput",
    "ScrollBy",
    "ScrollDirection",
    "ScrollInput",
    "SessionStateOutput",
    "StartSessionInput",
    "StartSessionOutput",
    "ToolResult",
    "TypeTextInput",
    "__version__",
    "get_binary_path",
    "run_cua_driver",
]
