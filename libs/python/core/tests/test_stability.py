"""Unit tests for HTTP request stability metrics.

Tests the in-memory stability tracker, score computation, and
record/report functions. All external dependencies are mocked.
"""

import sys
from types import ModuleType
from unittest.mock import MagicMock, patch

import pytest

# Provide a stub 'core' module so posthog.py can import __version__
if "core" not in sys.modules:
    _stub = ModuleType("core")
    _stub.__version__ = "0.0.0-test"
    sys.modules["core"] = _stub

from cua_core.telemetry.stability import (
    StabilitySnapshot,
    _StabilityTracker,
    get_stability_scores,
    record_request_stability,
    report_stability,
    reset_tracker,
)


@pytest.fixture(autouse=True)
def _clean_tracker():
    """Reset the module-level tracker before and after each test."""
    reset_tracker()
    yield
    reset_tracker()


class TestStabilityTracker:
    """Test the _StabilityTracker class directly."""

    def test_empty_snapshot(self):
        tracker = _StabilityTracker()
        snap = tracker.snapshot()
        assert snap.total_requests == 0
        assert snap.success_rate == 1.0
        assert snap.churn_rate == 0.0
        assert snap.latency_compliance_rate == 1.0

    def test_single_successful_request(self):
        tracker = _StabilityTracker()
        tracker.record(status_code=200, duration_seconds=0.5)
        snap = tracker.snapshot()
        assert snap.total_requests == 1
        assert snap.successful_requests == 1
        assert snap.failed_requests == 0
        assert snap.success_rate == 1.0
        assert snap.avg_latency_seconds == 0.5

    def test_single_failed_request(self):
        tracker = _StabilityTracker()
        tracker.record(status_code=500, duration_seconds=1.0)
        snap = tracker.snapshot()
        assert snap.total_requests == 1
        assert snap.successful_requests == 0
        assert snap.failed_requests == 1
        assert snap.churned_requests == 1
        assert snap.success_rate == 0.0
        assert snap.churn_rate == 1.0

    def test_connection_error(self):
        tracker = _StabilityTracker()
        tracker.record(status_code=0, duration_seconds=2.0, error="ConnectionError")
        snap = tracker.snapshot()
        assert snap.total_requests == 1
        assert snap.failed_requests == 1
        assert snap.churned_requests == 1
        assert snap.success_rate == 0.0

    def test_slow_successful_request_is_churned(self):
        """A request that succeeds but exceeds the latency target counts as churned."""
        tracker = _StabilityTracker()
        # Default latency target is 5.0s
        tracker.record(status_code=200, duration_seconds=6.0)
        snap = tracker.snapshot()
        assert snap.successful_requests == 1
        assert snap.failed_requests == 0
        assert snap.churned_requests == 1  # slow = churned
        assert snap.latency_compliance_rate == 0.0

    def test_mixed_requests(self):
        tracker = _StabilityTracker()
        # 3 successful, fast
        for _ in range(3):
            tracker.record(status_code=200, duration_seconds=0.1)
        # 1 successful, slow (churned)
        tracker.record(status_code=200, duration_seconds=10.0)
        # 1 failed (churned)
        tracker.record(status_code=500, duration_seconds=0.2)

        snap = tracker.snapshot()
        assert snap.total_requests == 5
        assert snap.successful_requests == 4
        assert snap.failed_requests == 1
        assert snap.churned_requests == 2  # 1 slow + 1 failed
        assert snap.success_rate == pytest.approx(0.8)
        assert snap.churn_rate == pytest.approx(0.4)
        assert snap.latency_compliance_rate == pytest.approx(0.8)

    def test_latency_percentiles(self):
        tracker = _StabilityTracker()
        # Record 100 requests with latencies 0.01, 0.02, ..., 1.00
        for i in range(1, 101):
            tracker.record(status_code=200, duration_seconds=i * 0.01)

        snap = tracker.snapshot()
        assert snap.p50_latency_seconds == pytest.approx(0.50, abs=0.02)
        assert snap.p95_latency_seconds == pytest.approx(0.95, abs=0.02)
        assert snap.p99_latency_seconds == pytest.approx(0.99, abs=0.02)

    def test_reset(self):
        tracker = _StabilityTracker()
        tracker.record(status_code=200, duration_seconds=0.5)
        tracker.reset()
        snap = tracker.snapshot()
        assert snap.total_requests == 0

    def test_4xx_counts_as_failure(self):
        tracker = _StabilityTracker()
        tracker.record(status_code=404, duration_seconds=0.1)
        snap = tracker.snapshot()
        assert snap.failed_requests == 1
        assert snap.churned_requests == 1

    def test_3xx_counts_as_success(self):
        tracker = _StabilityTracker()
        tracker.record(status_code=301, duration_seconds=0.1)
        snap = tracker.snapshot()
        assert snap.successful_requests == 1
        assert snap.churned_requests == 0


class TestPublicAPI:
    """Test the module-level public functions."""

    @patch("cua_core.telemetry.stability._tracker")
    @patch("cua_core.telemetry.otel.record_http_request")
    def test_record_request_stability_calls_otel(self, mock_otel, mock_tracker):
        record_request_stability(
            method="GET",
            url="https://api.cua.ai/v1/vms",
            status_code=200,
            duration_seconds=0.5,
        )
        mock_tracker.record.assert_called_once_with(200, 0.5, None)
        mock_otel.assert_called_once_with(
            method="GET",
            url="https://api.cua.ai/v1/vms",
            status_code=200,
            duration_seconds=0.5,
            error=None,
        )

    @patch("cua_core.telemetry.stability._tracker")
    @patch("cua_core.telemetry.otel.record_http_request")
    def test_record_request_stability_with_error(self, mock_otel, mock_tracker):
        record_request_stability(
            method="POST",
            url="https://api.cua.ai/v1/vms/test/start",
            status_code=0,
            duration_seconds=2.0,
            error="ConnectionError",
        )
        mock_tracker.record.assert_called_once_with(0, 2.0, "ConnectionError")

    def test_get_stability_scores_returns_snapshot(self):
        snap = get_stability_scores()
        assert isinstance(snap, StabilitySnapshot)
        assert snap.total_requests == 0

    @patch("cua_core.telemetry.posthog.record_event")
    @patch("cua_core.telemetry.posthog.is_telemetry_enabled", return_value=True)
    def test_report_stability_emits_posthog_event(self, mock_enabled, mock_record):
        """report_stability() emits an sdk_stability_report PostHog event."""
        from cua_core.telemetry.stability import _tracker

        _tracker.record(status_code=200, duration_seconds=0.1)
        _tracker.record(status_code=500, duration_seconds=0.2)

        result = report_stability()

        assert result["total_requests"] == 2
        assert result["successful_requests"] == 1
        assert result["failed_requests"] == 1

    def test_report_stability_empty_returns_empty_dict(self):
        result = report_stability()
        assert result == {}


class TestStabilitySnapshot:
    """Test StabilitySnapshot is frozen/immutable."""

    def test_snapshot_is_frozen(self):
        snap = StabilitySnapshot(
            total_requests=1,
            successful_requests=1,
            failed_requests=0,
            churned_requests=0,
            avg_latency_seconds=0.1,
            p50_latency_seconds=0.1,
            p95_latency_seconds=0.1,
            p99_latency_seconds=0.1,
            success_rate=1.0,
            churn_rate=0.0,
            latency_compliance_rate=1.0,
        )
        with pytest.raises(AttributeError):
            snap.total_requests = 99
