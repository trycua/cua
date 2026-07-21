"""Experimental UniFFI-backed imported SDK.

Importing this module loads the host-native ``cua-driver-sdk`` library bundled
with the package. Agent integrations should continue to use the MCP/CLI surface.
"""

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

__all__ = [name for name in globals() if not name.startswith("_")]
