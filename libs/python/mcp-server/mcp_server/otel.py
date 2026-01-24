"""
OpenTelemetry instrumentation for CUA MCP Server
Exports the Four Golden Signals: Latency, Traffic, Errors, Saturation

Exporter endpoint: otel.cua.ai
"""

import os
import time
import logging
from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import Any, Dict, Optional, Callable
from functools import wraps

# OpenTelemetry imports
from opentelemetry import metrics, trace
from opentelemetry.sdk.metrics import MeterProvider
from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.sdk.resources import Resource, SERVICE_NAME, SERVICE_VERSION
from opentelemetry.exporter.otlp.proto.http.metric_exporter import OTLPMetricExporter
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.metrics import Counter, Histogram, UpDownCounter

logger = logging.getLogger("mcp-server.otel")

# Configuration
OTEL_ENDPOINT = os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT", "https://otel.cua.ai")
SERVICE_NAME_VALUE = "cua-mcp-server"
SERVICE_VERSION_VALUE = os.getenv("CUA_MCP_SERVER_VERSION", "0.1.16")
METRIC_PREFIX = "cua.mcp_server"

# Global state
_initialized = False
_meter = None
_tracer = None


@dataclass
class GoldenSignalsMetrics:
    """Container for the Four Golden Signals metrics."""

    # LATENCY - Request duration histograms
    tool_execution_duration: Histogram = None
    session_operation_duration: Histogram = None
    task_duration: Histogram = None

    # TRAFFIC - Request counters
    tool_calls_total: Counter = None
    sessions_created_total: Counter = None
    tasks_total: Counter = None
    messages_processed_total: Counter = None

    # ERRORS - Error counters
    errors_total: Counter = None
    tool_errors_total: Counter = None
    session_errors_total: Counter = None
    task_errors_total: Counter = None

    # SATURATION - Resource utilization gauges (UpDownCounter for concurrent tracking)
    active_sessions: UpDownCounter = None
    active_tasks: UpDownCounter = None
    concurrent_tool_calls: UpDownCounter = None
    computer_pool_size: UpDownCounter = None


_metrics: Optional[GoldenSignalsMetrics] = None


def initialize_otel() -> GoldenSignalsMetrics:
    """
    Initialize OpenTelemetry SDK with OTLP exporters for the Four Golden Signals.

    Returns:
        GoldenSignalsMetrics instance with all metrics initialized.
    """
    global _initialized, _meter, _tracer, _metrics

    if _initialized and _metrics is not None:
        return _metrics

    logger.info(f"Initializing OpenTelemetry for {SERVICE_NAME_VALUE} -> {OTEL_ENDPOINT}")

    # Create resource describing this service
    resource = Resource.create({
        SERVICE_NAME: SERVICE_NAME_VALUE,
        SERVICE_VERSION: SERVICE_VERSION_VALUE,
        "service.namespace": "cua",
        "deployment.environment": os.getenv("CUA_ENVIRONMENT", "production"),
    })

    # Set up tracing
    trace_exporter = OTLPSpanExporter(
        endpoint=f"{OTEL_ENDPOINT}/v1/traces",
    )
    tracer_provider = TracerProvider(resource=resource)
    tracer_provider.add_span_processor(BatchSpanProcessor(trace_exporter))
    trace.set_tracer_provider(tracer_provider)
    _tracer = trace.get_tracer(SERVICE_NAME_VALUE, SERVICE_VERSION_VALUE)

    # Set up metrics
    metric_exporter = OTLPMetricExporter(
        endpoint=f"{OTEL_ENDPOINT}/v1/metrics",
    )
    metric_reader = PeriodicExportingMetricReader(
        metric_exporter,
        export_interval_millis=15000,  # Export every 15 seconds
    )
    meter_provider = MeterProvider(resource=resource, metric_readers=[metric_reader])
    metrics.set_meter_provider(meter_provider)
    _meter = metrics.get_meter(SERVICE_NAME_VALUE, SERVICE_VERSION_VALUE)

    # Initialize the Four Golden Signals metrics
    _metrics = GoldenSignalsMetrics(
        # ============================================
        # LATENCY - Time to service requests
        # ============================================
        tool_execution_duration=_meter.create_histogram(
            f"{METRIC_PREFIX}.tool.duration",
            description="Duration of tool executions in milliseconds",
            unit="ms",
        ),
        session_operation_duration=_meter.create_histogram(
            f"{METRIC_PREFIX}.session.operation_duration",
            description="Duration of session operations in milliseconds",
            unit="ms",
        ),
        task_duration=_meter.create_histogram(
            f"{METRIC_PREFIX}.task.duration",
            description="Duration of CUA tasks in milliseconds",
            unit="ms",
        ),

        # ============================================
        # TRAFFIC - Request volume
        # ============================================
        tool_calls_total=_meter.create_counter(
            f"{METRIC_PREFIX}.tool.calls_total",
            description="Total number of tool calls",
        ),
        sessions_created_total=_meter.create_counter(
            f"{METRIC_PREFIX}.sessions.created_total",
            description="Total number of sessions created",
        ),
        tasks_total=_meter.create_counter(
            f"{METRIC_PREFIX}.tasks.total",
            description="Total number of CUA tasks executed",
        ),
        messages_processed_total=_meter.create_counter(
            f"{METRIC_PREFIX}.messages.processed_total",
            description="Total number of messages processed",
        ),

        # ============================================
        # ERRORS - Failure rates
        # ============================================
        errors_total=_meter.create_counter(
            f"{METRIC_PREFIX}.errors.total",
            description="Total number of errors",
        ),
        tool_errors_total=_meter.create_counter(
            f"{METRIC_PREFIX}.tool.errors_total",
            description="Total number of tool execution errors",
        ),
        session_errors_total=_meter.create_counter(
            f"{METRIC_PREFIX}.session.errors_total",
            description="Total number of session errors",
        ),
        task_errors_total=_meter.create_counter(
            f"{METRIC_PREFIX}.task.errors_total",
            description="Total number of task errors",
        ),

        # ============================================
        # SATURATION - Resource utilization
        # ============================================
        active_sessions=_meter.create_up_down_counter(
            f"{METRIC_PREFIX}.sessions.active",
            description="Number of active sessions",
        ),
        active_tasks=_meter.create_up_down_counter(
            f"{METRIC_PREFIX}.tasks.active",
            description="Number of active tasks",
        ),
        concurrent_tool_calls=_meter.create_up_down_counter(
            f"{METRIC_PREFIX}.tool.concurrent",
            description="Number of concurrent tool calls",
        ),
        computer_pool_size=_meter.create_up_down_counter(
            f"{METRIC_PREFIX}.computer_pool.size",
            description="Current size of the computer pool",
        ),
    )

    _initialized = True
    logger.info("OpenTelemetry initialization complete")
    return _metrics


def get_metrics() -> GoldenSignalsMetrics:
    """Get the metrics instance, initializing if needed."""
    global _metrics
    if _metrics is None:
        return initialize_otel()
    return _metrics


def get_tracer():
    """Get the tracer instance, initializing if needed."""
    global _tracer
    if _tracer is None:
        initialize_otel()
    return _tracer


# ============================================
# Recording Functions
# ============================================

def record_tool_call(
    tool_name: str,
    duration_ms: float,
    success: bool,
    error_type: Optional[str] = None,
    session_id: Optional[str] = None,
) -> None:
    """
    Record a tool execution with all four golden signals.

    Args:
        tool_name: Name of the tool executed
        duration_ms: Execution duration in milliseconds
        success: Whether the tool execution succeeded
        error_type: Type of error if not successful
        session_id: Optional session ID for correlation
    """
    m = get_metrics()
    attributes = {
        "tool_name": tool_name,
        "success": str(success).lower(),
    }
    if session_id:
        attributes["session_id"] = session_id

    # Latency
    m.tool_execution_duration.record(duration_ms, attributes)

    # Traffic
    m.tool_calls_total.add(1, attributes)

    # Errors
    if not success:
        error_attrs = {**attributes, "error_type": error_type or "unknown"}
        m.tool_errors_total.add(1, error_attrs)
        m.errors_total.add(1, error_attrs)


def record_task(
    task_id: str,
    duration_ms: float,
    success: bool,
    error_type: Optional[str] = None,
    session_id: Optional[str] = None,
    model_name: Optional[str] = None,
) -> None:
    """
    Record a CUA task execution with all four golden signals.

    Args:
        task_id: Unique task identifier
        duration_ms: Task duration in milliseconds
        success: Whether the task succeeded
        error_type: Type of error if not successful
        session_id: Optional session ID for correlation
        model_name: Name of the model used
    """
    m = get_metrics()
    attributes = {
        "success": str(success).lower(),
    }
    if session_id:
        attributes["session_id"] = session_id
    if model_name:
        attributes["model_name"] = model_name

    # Latency
    m.task_duration.record(duration_ms, attributes)

    # Traffic
    m.tasks_total.add(1, attributes)

    # Errors
    if not success:
        error_attrs = {**attributes, "error_type": error_type or "unknown"}
        m.task_errors_total.add(1, error_attrs)
        m.errors_total.add(1, error_attrs)


def record_session_created(session_id: str) -> None:
    """Record a new session being created."""
    m = get_metrics()
    m.sessions_created_total.add(1, {"session_id": session_id})
    m.active_sessions.add(1)


def record_session_closed(session_id: str) -> None:
    """Record a session being closed."""
    m = get_metrics()
    m.active_sessions.add(-1)


def record_session_error(session_id: str, error_type: str) -> None:
    """Record a session error."""
    m = get_metrics()
    m.session_errors_total.add(1, {"session_id": session_id, "error_type": error_type})
    m.errors_total.add(1, {"error_type": error_type, "source": "session"})


def update_saturation(
    active_tasks_delta: int = 0,
    concurrent_tools_delta: int = 0,
    pool_size_delta: int = 0,
) -> None:
    """
    Update saturation metrics.

    Args:
        active_tasks_delta: Change in active tasks (+1 or -1)
        concurrent_tools_delta: Change in concurrent tool calls (+1 or -1)
        pool_size_delta: Change in computer pool size (+1 or -1)
    """
    m = get_metrics()
    if active_tasks_delta != 0:
        m.active_tasks.add(active_tasks_delta)
    if concurrent_tools_delta != 0:
        m.concurrent_tool_calls.add(concurrent_tools_delta)
    if pool_size_delta != 0:
        m.computer_pool_size.add(pool_size_delta)


# ============================================
# Decorators and Context Managers
# ============================================

@contextmanager
def timed_tool_execution(tool_name: str, session_id: Optional[str] = None):
    """
    Context manager for timing tool executions.

    Usage:
        with timed_tool_execution("screenshot_cua", session_id="abc123") as ctx:
            # execute tool
            ctx["success"] = True  # or False on error
            ctx["error_type"] = "timeout"  # if error
    """
    m = get_metrics()
    m.concurrent_tool_calls.add(1)

    start_time = time.perf_counter()
    context = {"success": True, "error_type": None}

    try:
        yield context
    except Exception as e:
        context["success"] = False
        context["error_type"] = type(e).__name__
        raise
    finally:
        duration_ms = (time.perf_counter() - start_time) * 1000
        m.concurrent_tool_calls.add(-1)
        record_tool_call(
            tool_name=tool_name,
            duration_ms=duration_ms,
            success=context["success"],
            error_type=context.get("error_type"),
            session_id=session_id,
        )


@contextmanager
def timed_task(task_id: str, session_id: Optional[str] = None, model_name: Optional[str] = None):
    """
    Context manager for timing CUA task executions.

    Usage:
        with timed_task("task-123", session_id="abc", model_name="claude") as ctx:
            # execute task
            ctx["success"] = True
    """
    m = get_metrics()
    m.active_tasks.add(1)

    start_time = time.perf_counter()
    context = {"success": True, "error_type": None}

    try:
        yield context
    except Exception as e:
        context["success"] = False
        context["error_type"] = type(e).__name__
        raise
    finally:
        duration_ms = (time.perf_counter() - start_time) * 1000
        m.active_tasks.add(-1)
        record_task(
            task_id=task_id,
            duration_ms=duration_ms,
            success=context["success"],
            error_type=context.get("error_type"),
            session_id=session_id,
            model_name=model_name,
        )


def instrument_tool(tool_name: str):
    """
    Decorator to instrument a tool function with OTEL metrics.

    Usage:
        @instrument_tool("screenshot_cua")
        async def screenshot_cua(ctx: Context, session_id: str = None):
            ...
    """
    def decorator(func: Callable):
        @wraps(func)
        async def async_wrapper(*args, **kwargs):
            session_id = kwargs.get("session_id")
            with timed_tool_execution(tool_name, session_id):
                return await func(*args, **kwargs)

        @wraps(func)
        def sync_wrapper(*args, **kwargs):
            session_id = kwargs.get("session_id")
            with timed_tool_execution(tool_name, session_id):
                return func(*args, **kwargs)

        # Return appropriate wrapper based on function type
        import asyncio
        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        return sync_wrapper

    return decorator


# ============================================
# Shutdown
# ============================================

def shutdown_otel() -> None:
    """Gracefully shutdown OpenTelemetry exporters."""
    global _initialized

    if not _initialized:
        return

    logger.info("Shutting down OpenTelemetry...")

    try:
        # Flush and shutdown tracer
        tracer_provider = trace.get_tracer_provider()
        if hasattr(tracer_provider, "shutdown"):
            tracer_provider.shutdown()

        # Flush and shutdown metrics
        meter_provider = metrics.get_meter_provider()
        if hasattr(meter_provider, "shutdown"):
            meter_provider.shutdown()

        logger.info("OpenTelemetry shutdown complete")
    except Exception as e:
        logger.error(f"Error during OpenTelemetry shutdown: {e}")

    _initialized = False
