"""This module provides the core telemetry functionality for Cua libraries.

It provides a low-overhead way to collect anonymous usage data via PostHog
and operational metrics via OpenTelemetry.

Stability metrics are collected automatically when HTTP requests are
instrumented via :func:`create_trace_config` (for ``aiohttp``) or
:func:`record_request_stability` (manual recording).
"""

# OpenTelemetry instrumentation for Four Golden Signals
from cua_core.telemetry.otel import (
    create_span,
    instrument_async,
    instrument_sync,
    is_otel_enabled,
    record_error,
    record_http_request,
    record_operation,
    record_tokens,
    track_concurrent,
)
from cua_core.telemetry.posthog import (
    destroy_telemetry_client,
    is_telemetry_enabled,
    record_event,
)

# HTTP stability metrics
from cua_core.telemetry.stability import (
    StabilitySnapshot,
    create_trace_config,
    get_stability_scores,
    record_request_stability,
    report_stability,
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
    "record_http_request",
    "record_tokens",
    "track_concurrent",
    "create_span",
    "instrument_async",
    "instrument_sync",
    # Stability metrics
    "StabilitySnapshot",
    "record_request_stability",
    "get_stability_scores",
    "report_stability",
    "create_trace_config",
]
