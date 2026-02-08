"""Agent telemetry for tracking anonymous usage and feature usage."""

import logging
import os
import platform
import sys
from typing import Any, Callable, Dict

# Import the core telemetry module
TELEMETRY_AVAILABLE = False


# Local fallbacks in case core telemetry isn't available
def _noop(*args: Any, **kwargs: Any) -> None:
    """No-op function for when telemetry is not available."""
    pass


# Define default functions with unique names to avoid shadowing
_default_record_event = _noop
_default_increment_counter = _noop
_default_set_dimension = _noop
_default_get_telemetry_client = lambda: None
_default_flush = _noop
_default_is_telemetry_enabled = lambda: False
_default_is_telemetry_globally_disabled = lambda: True

# Set the actual functions to the defaults initially
record_event = _default_record_event
increment_counter = _default_increment_counter
set_dimension = _default_set_dimension
get_telemetry_client = _default_get_telemetry_client
flush = _default_flush
is_telemetry_enabled = _default_is_telemetry_enabled
is_telemetry_globally_disabled = _default_is_telemetry_globally_disabled

logger = logging.getLogger("agent.telemetry")

try:
    # Import from core telemetry
    from core.telemetry import flush as core_flush
    from core.telemetry import get_telemetry_client as core_get_telemetry_client
    from core.telemetry import increment as core_increment
    from core.telemetry import is_telemetry_enabled as core_is_telemetry_enabled
    from core.telemetry import (
        is_telemetry_globally_disabled as core_is_telemetry_globally_disabled,
    )
    from core.telemetry import record_event as core_record_event

    # Override the default functions with actual implementations
    record_event = core_record_event
    get_telemetry_client = core_get_telemetry_client
    flush = core_flush
    is_telemetry_enabled = core_is_telemetry_enabled
    is_telemetry_globally_disabled = core_is_telemetry_globally_disabled

    def increment_counter(counter_name: str, value: int = 1) -> None:
        """Wrapper for increment to maintain backward compatibility."""
        if is_telemetry_enabled():
            core_increment(counter_name, value)

    def set_dimension(name: str, value: Any) -> None:
        """Set a dimension that will be attached to all events."""
        logger.debug(f"Setting dimension {name}={value}")

    TELEMETRY_AVAILABLE = True
    logger.info("Successfully imported telemetry")
except ImportError as e:
    logger.warning(f"Could not import telemetry: {e}")
    logger.debug("Telemetry not available, using no-op functions")

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
    global TELEMETRY_AVAILABLE, record_event, increment_counter, get_telemetry_client, flush, is_telemetry_enabled, is_telemetry_globally_disabled

    # Check if globally disabled using core function
    if TELEMETRY_AVAILABLE and is_telemetry_globally_disabled():
        logger.info("Telemetry is globally disabled via environment variable - cannot enable")
        return False

    # Already enabled
    if TELEMETRY_AVAILABLE:
        return True

    # Try to import and enable
    try:
        from core.telemetry import (
            flush,
            get_telemetry_client,
            increment,
            is_telemetry_globally_disabled,
            record_event,
        )

        # Check again after import
        if is_telemetry_globally_disabled():
            logger.info("Telemetry is globally disabled via environment variable - cannot enable")
            return False

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


def record_agent_initialization() -> None:
    """Record when an agent instance is initialized."""
    if TELEMETRY_AVAILABLE and is_telemetry_enabled():
        record_event("agent_initialized", SYSTEM_INFO)

        # Set dimensions that will be attached to all events
        set_dimension("os", SYSTEM_INFO["os"])
        set_dimension("os_version", SYSTEM_INFO["os_version"])
        set_dimension("python_version", SYSTEM_INFO["python_version"])
