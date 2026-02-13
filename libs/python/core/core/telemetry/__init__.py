"""This module provides the core telemetry functionality for Cua libraries.

It provides a low-overhead way to collect anonymous usage data via PostHog
and operational metrics via OpenTelemetry.
"""

# OpenTelemetry instrumentation for Four Golden Signals
from core.telemetry.otel import (
    create_span,
    instrument_async,
    instrument_sync,
    is_otel_enabled,
    record_error,
    record_operation,
    record_tokens,
    track_concurrent,
)
from core.telemetry.posthog import (
    destroy_telemetry_client,
    is_telemetry_enabled,
    record_event,
)

__all__ = [
    # PostHog (product analytics)
    "record_event",
    "is_telemetry_enabled",
    "destroy_telemetry_client",
    # OpenTelemetry (operational metrics)
    "is_otel_enabled",
    "record_operation",
    "record_error",
    "record_tokens",
    "track_concurrent",
    "create_span",
    "instrument_async",
    "instrument_sync",
]
