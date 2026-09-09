"""Rust-backed SDK and binary wrapper for Cua Driver client applications.

Agents should configure the bundled ``cua-driver mcp`` executable directly
through their runtime's MCP client instead of importing a language MCP facade.
"""

__version__ = "0.24.0"  # x-release-please-version

from ._native import (
    ActionCompletion,
    ConfiguredDriverOptions,
    CuaDriver as _NativeCuaDriver,
    CuaDriverSession,
    DriverActivityEvent,
    DriverActivityKind,
    DriverActivityObserver,
    DriverActivityObserverImpl,
    DriverAuthorizationAction,
    DriverAuthorizationDecision,
    DriverAuthorizationHost,
    DriverAuthorizationHostError,
    DriverAuthorizationHostImpl,
    DriverAuthorizationRequest,
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
    PrivateWorkerOptions,
    RuntimeAuthorizationOptions,
    SdkClientKind,
    SessionPermissionMode,
    ToolResult,
    TrustedSessionOptions,
    create_trusted_session,
    current_mac_os_permission_status,
    open_mac_os_screen_recording_settings,
    request_mac_os_permissions,
)
from ._native_contract import (
    ActionDelivery,
    ActionDeliveryMode,
    ActionEffect,
    ActionEscalation,
    ActionEscalationReason,
    ActionEscalationTarget,
    ActionEvidence,
    ActionEvidenceKind,
    ActionResult,
    ActionRoute,
    BoundsExpectation,
    CaptureScope,
    ClickButton,
    ClickInput,
    CursorAction,
    CursorReducedMotion,
    CursorThemeSelection,
    DesktopScope,
    DragInput,
    ElementPredicate,
    ElementSelector,
    EndSessionInput,
    EndSessionOutput,
    EffectiveScope,
    EscalateSessionInput,
    EscalationReason,
    GetAgentCursorStateInput,
    GetCursorPositionInput,
    GetDesktopStateInput,
    GetScreenSizeInput,
    GetSessionStateInput,
    HotkeyInput,
    InvokeMenuInput,
    MoveCursorInput,
    Platform,
    PredicateOutcome,
    PressKeyInput,
    ScrollBy,
    ScrollDirection,
    ScrollInput,
    SessionStateOutput,
    SetAgentCursorEnabledInput,
    SetAgentCursorMotionInput,
    SetAgentCursorThemeInput,
    SetWindowFrameInput,
    StartSessionInput,
    StartSessionOutput,
    StatePredicate,
    TypeTextInput,
    UnknownReason,
    VerificationStatus,
    VerifyStateInput,
    VerifyStateOutput,
    WindowPredicate,
)
from .wrapper import get_binary_path, run_cua_driver


def _connect_python_sdk(cls, socket_path):
    """Preserve ``CuaDriver.connect`` while tagging the imported runtime."""

    return cls.connect_with_client_kind(socket_path, SdkClientKind.PYTHON)


def _create_python_sdk(cls, options=None):
    """Create the canonical same-process SDK runtime for Python."""

    return cls.create_with_client_kind(options, SdkClientKind.PYTHON)


def _create_configured_python_sdk(cls, options):
    """Create a trusted configured runtime tagged as a Python SDK host."""

    return cls.create_configured_with_client_kind(options, SdkClientKind.PYTHON)


def _create_configured_with_authorization_host_python_sdk(cls, options, host):
    """Create a runtime with a trusted authorization callback."""

    return cls.create_configured_with_authorization_host_and_client_kind(
        options, host, SdkClientKind.PYTHON
    )

def _create_configured_with_activity_observer_python_sdk(cls, options, observer):
    """Create a runtime with a content-free activity observer."""

    return cls.create_configured_with_activity_observer_and_client_kind(
        options, observer, SdkClientKind.PYTHON
    )


def _create_configured_with_host_integrations_python_sdk(cls, options, host, observer):
    """Create a runtime with both trusted host integrations."""

    return cls.create_configured_with_host_integrations_and_client_kind(
        options, host, observer, SdkClientKind.PYTHON
    )


def _create_private_worker_python_sdk(cls, options):
    """Create a supervised worker runtime tagged as a Python SDK host."""

    return cls.create_private_worker_with_client_kind(options, SdkClientKind.PYTHON)


_NativeCuaDriver.connect = classmethod(_connect_python_sdk)
_NativeCuaDriver.create = classmethod(_create_python_sdk)
_NativeCuaDriver.create_configured = classmethod(_create_configured_python_sdk)
_NativeCuaDriver.create_configured_with_authorization_host = classmethod(
    _create_configured_with_authorization_host_python_sdk
)
_NativeCuaDriver.create_configured_with_activity_observer = classmethod(
    _create_configured_with_activity_observer_python_sdk
)
_NativeCuaDriver.create_configured_with_host_integrations = classmethod(
    _create_configured_with_host_integrations_python_sdk
)
_NativeCuaDriver.create_private_worker = classmethod(_create_private_worker_python_sdk)
CuaDriver = _NativeCuaDriver

__all__ = [
    "ActionCompletion",
    "ActionDelivery",
    "ActionDeliveryMode",
    "ActionEffect",
    "ActionEscalation",
    "ActionEscalationReason",
    "ActionEscalationTarget",
    "ActionEvidence",
    "ActionEvidenceKind",
    "ActionResult",
    "ActionRoute",
    "BoundsExpectation",
    "CaptureScope",
    "ClickButton",
    "ClickInput",
    "ConfiguredDriverOptions",
    "CuaDriver",
    "CuaDriverSession",
    "CursorAction",
    "CursorReducedMotion",
    "CursorThemeSelection",
    "DesktopScope",
    "DragInput",
    "ElementPredicate",
    "ElementSelector",
    "DriverError",
    "DriverActivityEvent",
    "DriverActivityKind",
    "DriverActivityObserver",
    "DriverActivityObserverImpl",
    "DriverAuthorizationAction",
    "DriverAuthorizationDecision",
    "DriverAuthorizationHost",
    "DriverAuthorizationHostError",
    "DriverAuthorizationHostImpl",
    "DriverAuthorizationRequest",
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
    "GetAgentCursorStateInput",
    "GetDesktopStateInput",
    "GetScreenSizeInput",
    "GetSessionStateInput",
    "HotkeyInput",
    "InvokeMenuInput",
    "ImageContent",
    "MacOsPermissionStatus",
    "MoveCursorInput",
    "Platform",
    "PredicateOutcome",
    "PrivateWorkerOptions",
    "PressKeyInput",
    "RuntimeAuthorizationOptions",
    "ScrollBy",
    "ScrollDirection",
    "ScrollInput",
    "SessionStateOutput",
    "SessionPermissionMode",
    "SetAgentCursorEnabledInput",
    "SetAgentCursorMotionInput",
    "SetAgentCursorThemeInput",
    "SetWindowFrameInput",
    "StartSessionInput",
    "StartSessionOutput",
    "StatePredicate",
    "ToolResult",
    "TrustedSessionOptions",
    "TypeTextInput",
    "UnknownReason",
    "VerificationStatus",
    "VerifyStateInput",
    "VerifyStateOutput",
    "WindowPredicate",
    "__version__",
    "create_trusted_session",
    "current_mac_os_permission_status",
    "get_binary_path",
    "open_mac_os_screen_recording_settings",
    "request_mac_os_permissions",
    "run_cua_driver",
]
