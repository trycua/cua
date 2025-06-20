"""Computer telemetry for tracking anonymous usage and feature usage."""

import logging
import platform
from typing import Any

# Import the core telemetry module
TELEMETRY_AVAILABLE = False

try:
    from core.telemetry import (
        increment,
        is_telemetry_enabled,
        is_telemetry_globally_disabled,
        record_event,
    )

    def increment_counter(counter_name: str, value: int = 1) -> None:
        """Wrapper for increment to maintain backward compatibility."""
        if is_telemetry_enabled():
            increment(counter_name, value)

    def set_dimension(name: str, value: Any) -> None:
        """Set a dimension that will be attached to all events."""
        logger = logging.getLogger("computer.telemetry")
        logger.debug(f"Setting dimension {name}={value}")

    TELEMETRY_AVAILABLE = True
    logger = logging.getLogger("computer.telemetry")
    logger.info("Successfully imported telemetry")
except ImportError as e:
    logger = logging.getLogger("computer.telemetry")
    logger.warning(f"Could not import telemetry: {e}")
    TELEMETRY_AVAILABLE = False


# Local fallbacks in case core telemetry isn't available
def _noop(*args: Any, **kwargs: Any) -> None:
    """No-op function for when telemetry is not available."""
    pass


logger = logging.getLogger("computer.telemetry")

# If telemetry isn't available, use no-op functions
if not TELEMETRY_AVAILABLE:
    logger.debug("Telemetry not available, using no-op functions")
    record_event = _noop  # type: ignore
    increment_counter = _noop  # type: ignore
    set_dimension = _noop  # type: ignore
    get_telemetry_client = lambda: None  # type: ignore
    flush = _noop  # type: ignore
    is_telemetry_enabled = lambda: False  # type: ignore
    is_telemetry_globally_disabled = lambda: True  # type: ignore

# Get system info once to use in telemetry
SYSTEM_INFO = {
    "os": platform.system().lower(),
    "os_version": platform.release(),
    "python_version": platform.python_version(),
}


def enable_telemetry() -> bool:
    """Enable telemetry if available.

    Returns:
        bool: True if telemetry was successfully enabled, False otherwise
    """
    global TELEMETRY_AVAILABLE

    # Check if globally disabled using core function
    if TELEMETRY_AVAILABLE and is_telemetry_globally_disabled():
        logger.info("Telemetry is globally disabled via environment variable - cannot enable")
        return False

    # Already enabled
    if TELEMETRY_AVAILABLE:
        return True

    # Try to import and enable
    try:
        # Verify we can import core telemetry
        from core.telemetry import record_event  # type: ignore

        TELEMETRY_AVAILABLE = True
        logger.info("Telemetry successfully enabled")
        return True
    except ImportError as e:
        logger.warning(f"Could not enable telemetry: {e}")
        return False


def is_telemetry_enabled() -> bool:
    """Check if telemetry is enabled.

    Returns:
        bool: True if telemetry is enabled, False otherwise
    """
    # Use the core function if available, otherwise use our local flag
    if TELEMETRY_AVAILABLE:
        from core.telemetry import is_telemetry_enabled as core_is_enabled

        return core_is_enabled()
    return False


def record_computer_initialization() -> None:
    """Record when a computer instance is initialized."""
    if TELEMETRY_AVAILABLE and is_telemetry_enabled():
        record_event("computer_initialized", SYSTEM_INFO)

        # Set dimensions that will be attached to all events
        set_dimension("os", SYSTEM_INFO["os"])
        set_dimension("os_version", SYSTEM_INFO["os_version"])
        set_dimension("python_version", SYSTEM_INFO["python_version"])
