"""
OpenTelemetry instrumentation wrapper for computer interface.

Records metrics for computer actions including:
- Latency: Duration of each action
- Traffic: Count of actions by type
- Errors: Action failures
"""

import time
from typing import Any, Callable, Optional

from .interface.base import BaseComputerInterface

# Import OTEL functions - available when cua-core[telemetry] is installed
try:
    from core.telemetry import (
        add_breadcrumb,
        capture_exception,
        create_span,
        is_otel_enabled,
        record_error,
        record_operation,
    )

    OTEL_AVAILABLE = True
except ImportError:
    OTEL_AVAILABLE = False

    def is_otel_enabled() -> bool:
        return False


# Actions that should be instrumented
INSTRUMENTED_ACTIONS = {
    # Mouse actions
    "left_click",
    "right_click",
    "double_click",
    "move_cursor",
    "scroll",
    "drag",
    "mouse_down",
    "mouse_up",
    # Keyboard actions
    "type_text",
    "press_key",
    "hotkey",
    "key_down",
    "key_up",
    # Screen actions
    "screenshot",
    "get_screen_size",
    "get_cursor_position",
    # Clipboard
    "get_clipboard",
    "set_clipboard",
    # Shell/commands
    "run_shell_command",
    "run_command",
    "open_url",
    "open_file",
    "open_application",
    # File operations
    "file_exists",
    "directory_exists",
    "list_directory",
    "read_file",
    "write_file",
    # Accessibility
    "get_accessibility_tree",
    "find_element",
}


class OtelInterfaceWrapper:
    """
    Wrapper that instruments computer interface methods with OpenTelemetry.

    Records:
    - Duration of each action (latency)
    - Count of actions (traffic)
    - Errors by action type
    """

    def __init__(
        self,
        original_interface: BaseComputerInterface,
        os_type: str = "unknown",
    ):
        """
        Initialize the OTEL wrapper.

        Args:
            original_interface: The original computer interface
            os_type: The OS type (macos, linux, windows)
        """
        self._original_interface = original_interface
        self._os_type = os_type
        self._enabled = OTEL_AVAILABLE and is_otel_enabled()

    def __getattr__(self, name: str) -> Any:
        """
        Delegate attribute access to the original interface.

        For instrumented methods, wrap them to record metrics.
        """
        attr = getattr(self._original_interface, name)

        # Only instrument async methods that are in our list
        if name in INSTRUMENTED_ACTIONS and callable(attr):
            return self._wrap_method(name, attr)

        return attr

    def _wrap_method(self, name: str, method: Callable) -> Callable:
        """
        Wrap a method to record OTEL metrics.

        Args:
            name: Method name
            method: Original method

        Returns:
            Wrapped method that records metrics
        """
        async def instrumented(*args: Any, **kwargs: Any) -> Any:
            if not self._enabled:
                return await method(*args, **kwargs)

            start_time = time.perf_counter()
            status = "success"
            error_type = None

            # Add breadcrumb for debugging
            if OTEL_AVAILABLE:
                add_breadcrumb(
                    category="computer",
                    message=f"Computer action: {name}",
                    level="info",
                    data={"action": name, "os_type": self._os_type},
                )

            try:
                with create_span(
                    f"computer.{name}",
                    {"action": name, "os_type": self._os_type},
                ):
                    result = await method(*args, **kwargs)
                    return result

            except Exception as e:
                status = "error"
                error_type = type(e).__name__

                # Capture exception in Sentry
                if OTEL_AVAILABLE:
                    capture_exception(
                        e,
                        context={
                            "action": name,
                            "os_type": self._os_type,
                        },
                    )
                raise

            finally:
                duration = time.perf_counter() - start_time

                # Record operation metrics
                if OTEL_AVAILABLE:
                    record_operation(
                        operation=f"computer.action.{name}",
                        duration_seconds=duration,
                        status=status,
                        os_type=self._os_type,
                    )

                    if error_type:
                        record_error(
                            error_type=error_type,
                            operation=f"computer.action.{name}",
                        )

        return instrumented


def wrap_interface_with_otel(
    interface: BaseComputerInterface,
    os_type: str = "unknown",
) -> BaseComputerInterface:
    """
    Wrap a computer interface with OTEL instrumentation.

    Args:
        interface: The original interface
        os_type: The OS type for labeling

    Returns:
        The wrapped interface (or original if OTEL disabled)
    """
    if not OTEL_AVAILABLE or not is_otel_enabled():
        return interface

    return OtelInterfaceWrapper(interface, os_type)  # type: ignore
