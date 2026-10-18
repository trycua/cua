"""Telemetry module for cua-bench.

This module provides analytics for tracking feature usage, user workflows,
and system performance. All telemetry is routed through cua-core's PostHog
infrastructure for consistency across the CUA ecosystem.

See README.md in this directory for the events that are actually sent and
how to opt out. User-chosen values (task directory names, custom agents,
path- or URL-like models, error messages) are never sent.

Usage:
    from cua_bench.telemetry import record_event, track_command

    # Track CLI command usage
    @track_command
    def my_command(args):
        ...

    # Track custom events
    record_event("custom_event", {"property": "value"})

Environment Variables:
    CUA_TELEMETRY=0 or DO_NOT_TRACK=1: disable telemetry (off by default in CI)
    CUA_TELEMETRY_DEBUG: Set to "on" for PostHog client debug logging
"""

from .events import (  # Core tracking functions; Tier 1 events; Tier 2 events; Decorators
    count_bucket,
    flush_telemetry,
    is_telemetry_enabled,
    record_event,
    score_bucket,
    taskset_id,
    track_batch_job_started,
    track_batch_task_completed,
    track_bench_run_completed,
    track_command,
    track_command_async,
    track_command_invoked,
    track_dataset_processing_completed,
    track_task_evaluation_completed,
    track_task_execution_failed,
    track_task_execution_started,
    track_task_step_executed,
)

__all__ = [
    # Core
    "record_event",
    "is_telemetry_enabled",
    "flush_telemetry",
    # Tier 1
    "track_command_invoked",
    "track_task_execution_started",
    "track_task_evaluation_completed",
    "track_batch_job_started",
    # Tier 2
    "track_task_step_executed",
    "track_batch_task_completed",
    "track_dataset_processing_completed",
    "track_task_execution_failed",
    # Run level
    "track_bench_run_completed",
    "taskset_id",
    "score_bucket",
    "count_bucket",
    # Decorators
    "track_command",
    "track_command_async",
]
