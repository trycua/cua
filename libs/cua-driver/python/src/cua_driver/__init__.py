"""Rust-backed SDK and binary wrapper for Cua Driver client applications.

Agents should configure the bundled ``cua-driver mcp`` executable directly
through their runtime's MCP client instead of importing a language MCP facade.
"""

__version__ = "0.12.6"  # x-release-please-version

from ._native import (
    CuaDriver as _NativeCuaDriver,
    DriverError,
    DriverExecutionMode,
    DriverMetadata,
    DriverOptions,
    EmbeddedCuaDriverHost,
    EmbeddedDriverConnection,
    EmbeddedDriverError,
    EmbeddedDriverExit,
    EmbeddedDriverHostOptions,
    EmbeddedDriverHostState,
    EmbeddedEnvironmentVariable,
    EmbeddedMcpConfiguration,
    EmbeddedPermissionMode,
    ImageContent,
    MacOsPermissionStatus,
    SdkClientKind,
    ToolResult,
    current_mac_os_permission_status,
    open_mac_os_screen_recording_settings,
    request_mac_os_permissions,
)
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


def _connect_python_sdk(cls, socket_path):
    """Preserve ``CuaDriver.connect`` while tagging the imported runtime."""

    return cls.connect_with_client_kind(socket_path, SdkClientKind.PYTHON)


def _create_python_sdk(cls, options=None):
    """Create the canonical same-process SDK runtime for Python."""

    return cls.create_with_client_kind(options, SdkClientKind.PYTHON)


_NativeCuaDriver.connect = classmethod(_connect_python_sdk)
_NativeCuaDriver.create = classmethod(_create_python_sdk)
CuaDriver = _NativeCuaDriver

__all__ = [
    "CaptureScope",
    "ClickButton",
    "ClickInput",
    "CuaDriver",
    "DesktopScope",
    "DragInput",
    "DriverError",
    "DriverExecutionMode",
    "DriverMetadata",
    "DriverOptions",
    "EmbeddedCuaDriverHost",
    "EmbeddedDriverConnection",
    "EmbeddedDriverError",
    "EmbeddedDriverExit",
    "EmbeddedDriverHostOptions",
    "EmbeddedDriverHostState",
    "EmbeddedEnvironmentVariable",
    "EmbeddedMcpConfiguration",
    "EmbeddedPermissionMode",
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
    "MacOsPermissionStatus",
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
    "current_mac_os_permission_status",
    "get_binary_path",
    "open_mac_os_screen_recording_settings",
    "request_mac_os_permissions",
    "run_cua_driver",
]
