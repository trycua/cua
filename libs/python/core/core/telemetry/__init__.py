"""This module provides the core telemetry functionality for CUA libraries.

It provides a low-overhead way to collect anonymous usage data.
"""

from core.telemetry.posthog import (
    destroy_telemetry_client,
    is_telemetry_enabled,
    record_event,
)

__all__ = [
    "record_event",
    "is_telemetry_enabled",
    "destroy_telemetry_client",
]
