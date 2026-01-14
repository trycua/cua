"""This module provides the core telemetry functionality for Cua libraries.

It provides a low-overhead way to collect anonymous usage data via PostHog,
operational metrics via OpenTelemetry, and error tracking via Sentry.
"""

from core.telemetry.posthog import (
    destroy_telemetry_client,
    is_telemetry_enabled,
    record_event,
)

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

# Sentry error tracking
from core.telemetry.sentry import (
    add_breadcrumb,
    capture_exception,
    capture_message,
    flush,
    is_sentry_enabled,
    set_context,
    set_tag,
    set_user,
    start_transaction,
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
    # Sentry (error tracking)
    "is_sentry_enabled",
    "capture_exception",
    "capture_message",
    "add_breadcrumb",
    "set_user",
    "set_tag",
    "set_context",
    "start_transaction",
    "flush",
]
