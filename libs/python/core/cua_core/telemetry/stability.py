"""HTTP request stability metrics for CUA SDK.

Tracks API request success/failure rates, latency, and computes stability
scores for monitoring SDK reliability and customer experience.

Stability is measured through three key indicators:
- Success Rate: Percentage of requests completing with 2xx/3xx status
- Latency Compliance: Percentage of requests within the configurable latency target
- Churn Rate: Percentage of requests that failed OR exceeded latency target
  (inspired by "customer happiness" metrics in service orchestration)

Metrics are collected automatically when using the instrumented aiohttp
TraceConfig and reported via OpenTelemetry and PostHog.
"""

from __future__ import annotations

import atexit
import collections
import logging
import os
import threading
import time
from dataclasses import dataclass
from typing import Any, Dict, Optional

logger = logging.getLogger("core.telemetry.stability")

# Default latency target: requests exceeding this are counted as "churned"
DEFAULT_LATENCY_TARGET_SECONDS = 5.0

# Max number of latency samples to keep in memory for percentile calculation
_MAX_LATENCY_SAMPLES = 10_000


@dataclass(frozen=True)
class StabilitySnapshot:
    """Point-in-time stability metrics snapshot.

    Attributes:
        total_requests: Total HTTP requests recorded
        successful_requests: Requests with 2xx/3xx status and no connection error
        failed_requests: Requests with 4xx/5xx status or connection error
        churned_requests: Requests that failed OR exceeded latency target
        avg_latency_seconds: Mean request latency
        p50_latency_seconds: Median request latency
        p95_latency_seconds: 95th percentile request latency
        p99_latency_seconds: 99th percentile request latency
        success_rate: successful / total (0.0 to 1.0)
        churn_rate: churned / total (0.0 to 1.0)
        latency_compliance_rate: within-target / total (0.0 to 1.0)
    """

    total_requests: int
    successful_requests: int
    failed_requests: int
    churned_requests: int
    avg_latency_seconds: float
    p50_latency_seconds: float
    p95_latency_seconds: float
    p99_latency_seconds: float
    success_rate: float
    churn_rate: float
    latency_compliance_rate: float


class _StabilityTracker:
    """Thread-safe in-memory tracker for HTTP request stability metrics.

    Uses a bounded deque for latency samples to avoid unbounded memory growth
    in long-running processes.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        raw_latency_target = os.environ.get("CUA_LATENCY_TARGET_SECONDS")
        try:
            self._latency_target = (
                float(raw_latency_target)
                if raw_latency_target is not None
                else DEFAULT_LATENCY_TARGET_SECONDS
            )
        except (ValueError, TypeError):
            logger.warning(
                "Invalid CUA_LATENCY_TARGET_SECONDS=%r; using default %.1fs",
                raw_latency_target,
                DEFAULT_LATENCY_TARGET_SECONDS,
            )
            self._latency_target = DEFAULT_LATENCY_TARGET_SECONDS
        self._total = 0
        self._successful = 0
        self._failed = 0
        self._churned = 0
        self._latency_compliant = 0
        self._total_latency = 0.0
        self._latencies: collections.deque[float] = collections.deque(maxlen=_MAX_LATENCY_SAMPLES)

    def record(
        self,
        status_code: int,
        duration_seconds: float,
        error: Optional[str] = None,
    ) -> None:
        """Record a single HTTP request outcome."""
        with self._lock:
            self._total += 1
            self._total_latency += duration_seconds
            self._latencies.append(duration_seconds)

            is_success = error is None and 200 <= status_code < 400
            is_within_target = duration_seconds <= self._latency_target

            if is_success:
                self._successful += 1
            else:
                self._failed += 1

            if is_within_target:
                self._latency_compliant += 1

            # Churned = failed OR exceeded latency target
            if not is_success or not is_within_target:
                self._churned += 1

    def snapshot(self) -> StabilitySnapshot:
        """Return a point-in-time snapshot of stability metrics."""
        with self._lock:
            total = self._total
            if total == 0:
                return StabilitySnapshot(
                    total_requests=0,
                    successful_requests=0,
                    failed_requests=0,
                    churned_requests=0,
                    avg_latency_seconds=0.0,
                    p50_latency_seconds=0.0,
                    p95_latency_seconds=0.0,
                    p99_latency_seconds=0.0,
                    success_rate=1.0,
                    churn_rate=0.0,
                    latency_compliance_rate=1.0,
                )

            sorted_latencies = sorted(self._latencies)
            n = len(sorted_latencies)

            return StabilitySnapshot(
                total_requests=total,
                successful_requests=self._successful,
                failed_requests=self._failed,
                churned_requests=self._churned,
                avg_latency_seconds=self._total_latency / total,
                p50_latency_seconds=sorted_latencies[min(int(n * 0.50), n - 1)],
                p95_latency_seconds=sorted_latencies[min(int(n * 0.95), n - 1)],
                p99_latency_seconds=sorted_latencies[min(int(n * 0.99), n - 1)],
                success_rate=self._successful / total,
                churn_rate=self._churned / total,
                latency_compliance_rate=self._latency_compliant / total,
            )

    def reset(self) -> None:
        """Reset all tracked metrics. Primarily for testing."""
        with self._lock:
            self._total = 0
            self._successful = 0
            self._failed = 0
            self._churned = 0
            self._latency_compliant = 0
            self._total_latency = 0.0
            self._latencies.clear()


# ---------------------------------------------------------------------------
# Module-level singleton
# ---------------------------------------------------------------------------

_tracker = _StabilityTracker()
_atexit_registered = False
_atexit_lock = threading.Lock()


def _ensure_atexit() -> None:
    """Register an atexit handler (once) to report final stability scores."""
    global _atexit_registered
    if _atexit_registered:
        return
    with _atexit_lock:
        if _atexit_registered:
            return
        atexit.register(_report_on_exit)
        _atexit_registered = True


def _report_on_exit() -> None:
    """Emit a stability report to PostHog at process exit."""
    snapshot = _tracker.snapshot()
    if snapshot.total_requests == 0:
        return

    try:
        from cua_core.telemetry.posthog import is_telemetry_enabled, record_event

        if not is_telemetry_enabled():
            return

        record_event(
            "sdk_stability_report",
            _snapshot_to_dict(snapshot),
        )
        # Reset to avoid re-emitting the same data if called again
        _tracker.reset()
    except Exception as e:
        logger.debug(f"Failed to report stability metrics on exit: {e}")


def _snapshot_to_dict(snapshot: StabilitySnapshot) -> Dict[str, Any]:
    """Convert a StabilitySnapshot to a plain dict suitable for telemetry."""
    return {
        "total_requests": snapshot.total_requests,
        "successful_requests": snapshot.successful_requests,
        "failed_requests": snapshot.failed_requests,
        "churned_requests": snapshot.churned_requests,
        "avg_latency_seconds": round(snapshot.avg_latency_seconds, 4),
        "p50_latency_seconds": round(snapshot.p50_latency_seconds, 4),
        "p95_latency_seconds": round(snapshot.p95_latency_seconds, 4),
        "p99_latency_seconds": round(snapshot.p99_latency_seconds, 4),
        "success_rate": round(snapshot.success_rate, 4),
        "churn_rate": round(snapshot.churn_rate, 4),
        "latency_compliance_rate": round(snapshot.latency_compliance_rate, 4),
    }


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def record_request_stability(
    method: str,
    url: str,
    status_code: int,
    duration_seconds: float,
    error: Optional[str] = None,
) -> None:
    """Record an HTTP request for stability tracking.

    Records the request in both the in-memory stability tracker (for local
    score computation) and via OpenTelemetry (for real-time monitoring).

    Args:
        method: HTTP method (GET, POST, etc.)
        url: Full request URL
        status_code: HTTP response status code (0 if connection error)
        duration_seconds: Request duration in seconds
        error: Error type name if the request failed with an exception
    """
    _ensure_atexit()

    # Record in local tracker for score computation
    _tracker.record(status_code, duration_seconds, error)

    # Record in OTEL for real-time monitoring
    try:
        from cua_core.telemetry.otel import record_http_request

        record_http_request(
            method=method,
            url=url,
            status_code=status_code,
            duration_seconds=duration_seconds,
            error=error,
        )
    except Exception as e:
        logger.debug(f"Failed to record HTTP request in OTEL: {e}")


def get_stability_scores() -> StabilitySnapshot:
    """Get current stability scores as a snapshot.

    Returns a ``StabilitySnapshot`` with success rate, churn rate,
    latency percentiles, and other stability indicators.
    """
    return _tracker.snapshot()


def report_stability() -> Dict[str, Any]:
    """Report current stability scores via PostHog and return them.

    Call this at natural lifecycle boundaries (e.g., end of agent session)
    to emit a ``sdk_stability_report`` event.

    Returns:
        Dict with stability metric values, or empty dict if no requests
        have been recorded.
    """
    snapshot = _tracker.snapshot()
    if snapshot.total_requests == 0:
        return {}

    metrics = _snapshot_to_dict(snapshot)

    try:
        from cua_core.telemetry.posthog import is_telemetry_enabled, record_event

        if is_telemetry_enabled():
            record_event("sdk_stability_report", metrics)
            # Reset to avoid re-emitting overlapping totals on subsequent calls
            _tracker.reset()
    except Exception as e:
        logger.debug(f"Failed to report stability metrics via PostHog: {e}")

    return metrics


def create_trace_config() -> Any:
    """Create an ``aiohttp.TraceConfig`` that instruments HTTP requests.

    Attach this to ``aiohttp.ClientSession`` to automatically track all
    HTTP request metrics for stability monitoring.

    Example::

        from cua_core.telemetry.stability import create_trace_config

        trace_config = create_trace_config()
        async with aiohttp.ClientSession(
            trace_configs=[trace_config],
        ) as session:
            async with session.get(url) as resp:
                ...

    Returns:
        An ``aiohttp.TraceConfig`` with stability instrumentation hooks.
    """
    import aiohttp

    trace_config = aiohttp.TraceConfig()

    async def on_request_start(
        session: aiohttp.ClientSession,
        trace_config_ctx: Any,
        params: aiohttp.TraceRequestStartParams,
    ) -> None:
        trace_config_ctx.start_time = time.perf_counter()

    async def on_request_end(
        session: aiohttp.ClientSession,
        trace_config_ctx: Any,
        params: aiohttp.TraceRequestEndParams,
    ) -> None:
        duration = time.perf_counter() - trace_config_ctx.start_time
        record_request_stability(
            method=params.method,
            url=str(params.url),
            status_code=params.response.status,
            duration_seconds=duration,
        )

    async def on_request_exception(
        session: aiohttp.ClientSession,
        trace_config_ctx: Any,
        params: aiohttp.TraceRequestExceptionParams,
    ) -> None:
        duration = time.perf_counter() - trace_config_ctx.start_time
        record_request_stability(
            method=params.method,
            url=str(params.url),
            status_code=0,
            duration_seconds=duration,
            error=type(params.exception).__name__,
        )

    trace_config.on_request_start.append(on_request_start)
    trace_config.on_request_end.append(on_request_end)
    trace_config.on_request_exception.append(on_request_exception)

    return trace_config


def reset_tracker() -> None:
    """Reset the stability tracker. Intended for testing only."""
    _tracker.reset()
