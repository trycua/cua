"""OpenTelemetry instrumentation for CUA SDK.

Provides metrics and tracing for the Four Golden Signals:
- Latency: Operation duration histograms
- Traffic: Operation counters
- Errors: Error counters
- Saturation: Concurrent operation gauges
"""

from __future__ import annotations

import atexit
import logging
import os
import re
import time
from contextlib import contextmanager
from functools import wraps
from threading import Lock
from typing import Any, Callable, Dict, Generator, Optional, TypeVar

from cua_core.telemetry._config import sanitize_model_name, telemetry_enabled_from_env

logger = logging.getLogger("core.telemetry.otel")

# Type vars for decorator
F = TypeVar("F", bound=Callable[..., Any])

# Default OTEL endpoint
DEFAULT_OTEL_ENDPOINT = "https://otel.cua.ai"

# Lazy initialization state
_initialized = False
_init_failed = False
_init_lock = Lock()
_deprecation_warning_emitted = False
_deprecation_warning_lock = Lock()

# OTEL components (lazily initialized)
_meter: Optional[Any] = None
_tracer: Optional[Any] = None
_meter_provider: Optional[Any] = None
_tracer_provider: Optional[Any] = None

# Metrics (lazily initialized)
_operation_duration: Optional[Any] = None  # Histogram
_operations_total: Optional[Any] = None  # Counter
_errors_total: Optional[Any] = None  # Counter
_concurrent_operations: Optional[Any] = None  # UpDownCounter
_tokens_total: Optional[Any] = None  # Counter


def is_otel_enabled() -> bool:
    """Check if OpenTelemetry is enabled.

    Follows the shared Cua rules (``DO_NOT_TRACK``, ``CUA_TELEMETRY``, legacy
    ``CUA_TELEMETRY_ENABLED``/``CUA_TELEMETRY_DISABLED``, CI default off). See
    :mod:`cua_core.telemetry._config`. ``CUA_TELEMETRY_DISABLED`` is deprecated
    and emits a single warning when set.
    """
    import warnings

    global _deprecation_warning_emitted

    if os.environ.get("CUA_TELEMETRY_DISABLED", "") and not _deprecation_warning_emitted:
        with _deprecation_warning_lock:
            if not _deprecation_warning_emitted:
                warnings.warn(
                    "CUA_TELEMETRY_DISABLED is deprecated. Use CUA_TELEMETRY=0 instead.",
                    DeprecationWarning,
                    stacklevel=2,
                )
                _deprecation_warning_emitted = True

    return telemetry_enabled_from_env()


# Only these attribute keys may be attached to metrics. Everything else
# (step numbers, ids, names, paths) is dropped to avoid high cardinality and
# accidental personal data.
ALLOWED_ATTRIBUTE_KEYS = frozenset(
    {"operation", "status", "model", "os_type", "error_type", "token_type", "operation_type"}
)
_SAFE_VALUE = re.compile(r"^[A-Za-z0-9._:\-]{1,64}$")


def _safe_value(value: Any) -> str:
    """Coarse label value: short identifier-like strings only."""
    text = str(value)
    return text if _SAFE_VALUE.match(text) else "other"


def _build_attributes(
    base: Dict[str, Any], extra: Optional[Dict[str, Any]] = None
) -> Dict[str, str]:
    attributes: Dict[str, str] = {}
    for key, value in {**(extra or {}), **base}.items():
        if value is None or key not in ALLOWED_ATTRIBUTE_KEYS:
            continue
        if key == "model":
            attributes[key] = sanitize_model_name(str(value)) or "unknown"
        else:
            attributes[key] = _safe_value(value)
    return attributes


def _get_otel_endpoint() -> str:
    """Get the OTLP endpoint URL."""
    return os.environ.get("CUA_OTEL_ENDPOINT", DEFAULT_OTEL_ENDPOINT)


def _get_service_name() -> str:
    """Get the service name for OTEL."""
    return os.environ.get("CUA_OTEL_SERVICE_NAME", "cua-sdk")


def _initialize_otel() -> bool:
    """Initialize OpenTelemetry components.

    Returns True if initialization succeeded, False otherwise.
    Thread-safe via lock.
    """
    global _initialized, _init_failed, _meter, _tracer, _meter_provider, _tracer_provider
    global _operation_duration, _operations_total, _errors_total
    global _concurrent_operations, _tokens_total

    if _initialized:
        return True
    if _init_failed:
        return False

    with _init_lock:
        # Double-check after acquiring lock
        if _initialized:
            return True
        if _init_failed:
            return False

        if not is_otel_enabled():
            logger.debug("OpenTelemetry disabled")
            return False

        try:
            # Import OTEL packages lazily
            from opentelemetry.exporter.otlp.proto.http.metric_exporter import (
                OTLPMetricExporter,
            )
            from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
                OTLPSpanExporter,
            )
            from opentelemetry.sdk.metrics import MeterProvider
            from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
            from opentelemetry.sdk.resources import Resource
            from opentelemetry.sdk.trace import TracerProvider
            from opentelemetry.sdk.trace.export import BatchSpanProcessor

            # Create resource with service info
            resource = Resource.create(
                {
                    "service.name": _get_service_name(),
                    "service.version": _get_sdk_version(),
                }
            )

            endpoint = _get_otel_endpoint()

            # Set up metrics
            metric_exporter = OTLPMetricExporter(
                endpoint=f"{endpoint}/v1/metrics",
            )
            metric_reader = PeriodicExportingMetricReader(
                metric_exporter,
                export_interval_millis=60000,  # Export every 60 seconds
            )
            _meter_provider = MeterProvider(
                resource=resource,
                metric_readers=[metric_reader],
            )
            # Module-local provider: never installed as the global provider,
            # so host applications' own OpenTelemetry setup is not hijacked.
            _meter = _meter_provider.get_meter("cua-sdk", _get_sdk_version())

            # Set up tracing
            trace_exporter = OTLPSpanExporter(
                endpoint=f"{endpoint}/v1/traces",
            )
            _tracer_provider = TracerProvider(resource=resource)
            _tracer_provider.add_span_processor(BatchSpanProcessor(trace_exporter))
            _tracer = _tracer_provider.get_tracer("cua-sdk", _get_sdk_version())

            # Create metrics instruments
            _operation_duration = _meter.create_histogram(
                name="cua_sdk_operation_duration_seconds",
                description="Duration of SDK operations in seconds",
                unit="s",
            )

            _operations_total = _meter.create_counter(
                name="cua_sdk_operations_total",
                description="Total number of SDK operations",
                unit="1",
            )

            _errors_total = _meter.create_counter(
                name="cua_sdk_errors_total",
                description="Total number of SDK errors",
                unit="1",
            )

            _concurrent_operations = _meter.create_up_down_counter(
                name="cua_sdk_concurrent_operations",
                description="Number of concurrent SDK operations",
                unit="1",
            )

            _tokens_total = _meter.create_counter(
                name="cua_sdk_tokens_total",
                description="Total tokens consumed",
                unit="1",
            )

            # Register shutdown handler
            atexit.register(_shutdown_otel)

            _initialized = True
            logger.debug(f"OpenTelemetry initialized with endpoint: {endpoint}")
            return True

        except ImportError as e:
            _init_failed = True
            logger.debug(
                f"OpenTelemetry packages not installed: {e}. "
                "Install with: pip install opentelemetry-api opentelemetry-sdk "
                "opentelemetry-exporter-otlp-proto-http"
            )
            return False
        except Exception as e:
            _init_failed = True
            logger.debug(f"Failed to initialize OpenTelemetry: {type(e).__name__}")
            return False


def _shutdown_otel() -> None:
    """Shutdown OpenTelemetry providers gracefully."""
    global _meter_provider, _tracer_provider

    try:
        if _meter_provider is not None:
            _meter_provider.shutdown()
        if _tracer_provider is not None:
            _tracer_provider.shutdown()
        logger.debug("OpenTelemetry shutdown complete")
    except Exception as e:
        logger.debug(f"Error during OpenTelemetry shutdown: {e}")


def _get_sdk_version() -> str:
    """Get the CUA SDK version."""
    try:
        from cua_core import __version__

        return __version__
    except ImportError:
        return "unknown"


# --- Public API ---


def record_operation(
    operation: str,
    duration_seconds: float,
    status: str = "success",
    model: Optional[str] = None,
    os_type: Optional[str] = None,
    **extra_attributes: Any,
) -> None:
    """Record an operation metric (latency + traffic).

    Args:
        operation: Operation name (e.g., "agent.run", "computer.action.click")
        duration_seconds: Duration of the operation in seconds
        status: Operation status ("success" or "error")
        model: Model name if applicable
        os_type: OS type if applicable
        **extra_attributes: Additional attributes; only keys in
            ``ALLOWED_ATTRIBUTE_KEYS`` are kept, everything else is dropped
    """
    if not _initialize_otel():
        return

    attributes = _build_attributes(
        {"operation": operation, "status": status, "model": model, "os_type": os_type},
        extra_attributes,
    )

    try:
        if _operation_duration is not None:
            _operation_duration.record(duration_seconds, attributes)
        if _operations_total is not None:
            _operations_total.add(1, attributes)
    except Exception as e:
        logger.debug(f"Failed to record operation metric: {e}")


def record_error(
    error_type: str,
    operation: str,
    model: Optional[str] = None,
    **extra_attributes: Any,
) -> None:
    """Record an error metric.

    Args:
        error_type: Type of error (e.g., "api_error", "timeout", "computer_error")
        operation: Operation that failed
        model: Model name if applicable
        **extra_attributes: Additional attributes to record
    """
    if not _initialize_otel():
        return

    attributes = _build_attributes(
        {"error_type": error_type, "operation": operation, "model": model},
        extra_attributes,
    )

    try:
        if _errors_total is not None:
            _errors_total.add(1, attributes)
    except Exception as e:
        logger.debug(f"Failed to record error metric: {e}")


def record_tokens(
    prompt_tokens: int = 0,
    completion_tokens: int = 0,
    model: Optional[str] = None,
) -> None:
    """Record token usage metrics.

    Args:
        prompt_tokens: Number of prompt tokens
        completion_tokens: Number of completion tokens
        model: Model name
    """
    if not _initialize_otel():
        return

    try:
        if _tokens_total is not None:
            if prompt_tokens > 0:
                _tokens_total.add(
                    prompt_tokens,
                    _build_attributes({"token_type": "prompt", "model": model or "unknown"}),
                )
            if completion_tokens > 0:
                _tokens_total.add(
                    completion_tokens,
                    _build_attributes({"token_type": "completion", "model": model or "unknown"}),
                )
    except Exception as e:
        logger.debug(f"Failed to record token metric: {e}")


@contextmanager
def track_concurrent(operation_type: str) -> Generator[None, None, None]:
    """Context manager to track concurrent operations.

    Args:
        operation_type: Type of operation (e.g., "sessions", "runs")

    Example:
        with track_concurrent("agent_sessions"):
            # session is active
            pass
    """
    if not _initialize_otel():
        yield
        return

    attributes = _build_attributes({"operation_type": operation_type})

    try:
        if _concurrent_operations is not None:
            _concurrent_operations.add(1, attributes)
    except Exception as e:
        logger.debug(f"Failed to increment concurrent counter: {e}")

    try:
        yield
    finally:
        try:
            if _concurrent_operations is not None:
                _concurrent_operations.add(-1, attributes)
        except Exception as e:
            logger.debug(f"Failed to decrement concurrent counter: {e}")


@contextmanager
def create_span(
    name: str,
    attributes: Optional[Dict[str, Any]] = None,
) -> Generator[Any, None, None]:
    """Create a trace span context.

    Args:
        name: Span name
        attributes: Span attributes

    Yields:
        The span object (or None if tracing disabled)

    Example:
        with create_span("agent.run", {"model": "claude-3"}) as span:
            # do work
            if span:
                span.set_attribute("steps", 5)
    """
    if not _initialize_otel() or _tracer is None:
        yield None
        return

    _yielded = False
    try:
        span_attributes = _build_attributes(attributes or {})
        with _tracer.start_as_current_span(name, attributes=span_attributes) as span:
            _yielded = True
            yield span
    except Exception as e:
        if _yielded:
            raise
        logger.debug(f"Failed to create span: {e}")
        yield None


def instrument_async(
    operation: str,
    model_attr: Optional[str] = None,
    os_type_attr: Optional[str] = None,
) -> Callable[[F], F]:
    """Decorator to instrument an async function.

    Records duration, success/error status, and creates a trace span.

    Args:
        operation: Operation name for metrics
        model_attr: Attribute name to extract model from kwargs
        os_type_attr: Attribute name to extract os_type from kwargs

    Example:
        @instrument_async("agent.run", model_attr="model")
        async def run(self, prompt: str, model: str = "claude-3"):
            ...
    """

    def decorator(func: F) -> F:
        @wraps(func)
        async def wrapper(*args: Any, **kwargs: Any) -> Any:
            if not is_otel_enabled():
                return await func(*args, **kwargs)

            model = kwargs.get(model_attr) if model_attr else None
            os_type = kwargs.get(os_type_attr) if os_type_attr else None

            start_time = time.perf_counter()
            status = "success"
            error_type = None

            with create_span(operation, {"model": model, "os_type": os_type}):
                try:
                    result = await func(*args, **kwargs)
                    return result
                except Exception as e:
                    status = "error"
                    error_type = type(e).__name__
                    raise
                finally:
                    duration = time.perf_counter() - start_time
                    record_operation(
                        operation=operation,
                        duration_seconds=duration,
                        status=status,
                        model=model,
                        os_type=os_type,
                    )
                    if error_type:
                        record_error(
                            error_type=error_type,
                            operation=operation,
                            model=model,
                        )

        return wrapper  # type: ignore

    return decorator


def instrument_sync(
    operation: str,
    model_attr: Optional[str] = None,
    os_type_attr: Optional[str] = None,
) -> Callable[[F], F]:
    """Decorator to instrument a sync function.

    Records duration, success/error status, and creates a trace span.

    Args:
        operation: Operation name for metrics
        model_attr: Attribute name to extract model from kwargs
        os_type_attr: Attribute name to extract os_type from kwargs

    Example:
        @instrument_sync("computer.screenshot")
        def screenshot(self):
            ...
    """

    def decorator(func: F) -> F:
        @wraps(func)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            if not is_otel_enabled():
                return func(*args, **kwargs)

            model = kwargs.get(model_attr) if model_attr else None
            os_type = kwargs.get(os_type_attr) if os_type_attr else None

            start_time = time.perf_counter()
            status = "success"
            error_type = None

            with create_span(operation, {"model": model, "os_type": os_type}):
                try:
                    result = func(*args, **kwargs)
                    return result
                except Exception as e:
                    status = "error"
                    error_type = type(e).__name__
                    raise
                finally:
                    duration = time.perf_counter() - start_time
                    record_operation(
                        operation=operation,
                        duration_seconds=duration,
                        status=status,
                        model=model,
                        os_type=os_type,
                    )
                    if error_type:
                        record_error(
                            error_type=error_type,
                            operation=operation,
                            model=model,
                        )

        return wrapper  # type: ignore

    return decorator
