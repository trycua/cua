"""Telemetry events for cua-bench.

This module implements tracking events using cua-core's telemetry infrastructure.
Events are designed to help understand:
- What features are people using?
- How are people using these features?
- User segmentation based on usage patterns

All telemetry is routed through cua-core's PostHog client for consistency
across the CUA ecosystem.
"""

from __future__ import annotations

import functools
import logging
import os
import platform
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Optional

from core.telemetry import is_telemetry_enabled as _core_is_telemetry_enabled
from core.telemetry import record_event as _core_record_event
from core.telemetry.posthog import PostHogTelemetryClient

logger = logging.getLogger("cua_bench.telemetry")


def is_telemetry_enabled() -> bool:
    """Check if telemetry is enabled.

    Delegates to cua-core's telemetry check.
    """
    return _core_is_telemetry_enabled()


def _get_version() -> str:
    """Get cua-bench version."""
    try:
        from importlib import metadata

        return metadata.version("cua_bench")
    except Exception:
        return "dev"


def _get_common_properties() -> Dict[str, Any]:
    """Get common properties for all cua-bench events."""
    return {
        "bench_version": _get_version(),
        "python_version": platform.python_version(),
        "os": platform.system(),
        "os_version": platform.release(),
        "is_ci": "CI" in os.environ,
        "timestamp": datetime.now(timezone.utc).isoformat(),
    }


def record_event(event_name: str, properties: Optional[Dict[str, Any]] = None) -> None:
    """Record a telemetry event.

    Routes through cua-core's telemetry infrastructure.

    Args:
        event_name: Name of the event (e.g., "cb_command_invoked")
        properties: Optional dict of event properties
    """
    if not is_telemetry_enabled():
        return

    # Merge common properties with event-specific ones
    event_props = _get_common_properties()
    if properties:
        event_props.update(properties)

    try:
        _core_record_event(event_name, event_props)
        logger.debug(f"Recorded event: {event_name}")
    except Exception as e:
        logger.debug(f"Failed to record event {event_name}: {e}")


def flush_telemetry() -> None:
    """Flush pending telemetry events.

    Delegates to cua-core's PostHog client.
    """
    try:
        client = PostHogTelemetryClient.get_client()
        client.flush()
    except Exception as e:
        logger.debug(f"Failed to flush telemetry: {e}")


# =============================================================================
# Tier 1 Events (Core - Must Have)
# =============================================================================


def track_command_invoked(
    command: str,
    subcommand: Optional[str] = None,
    args: Optional[Dict[str, Any]] = None,
) -> None:
    """Track CLI command invocation.

    This is the primary event for understanding feature usage.

    Args:
        command: Main command (e.g., "run", "interact", "trace")
        subcommand: Optional subcommand (e.g., "task", "dataset", "list")
        args: Optional sanitized arguments (no sensitive data)
    """
    properties = {
        "command": command,
    }
    if subcommand:
        properties["subcommand"] = subcommand
    if args:
        # Only include safe args
        safe_keys = [
            "agent",
            "model",
            "max_steps",
            "platform",
            "provider_type",
            "max_parallel",
            "oracle",
            "wait",
            "debug",
        ]
        properties["args"] = {k: v for k, v in args.items() if k in safe_keys and v is not None}

    record_event("cb_command_invoked", properties)


def track_task_execution_started(
    env_name: str,
    task_index: int,
    *,
    provider_type: Optional[str] = None,
    os_type: Optional[str] = None,
    agent: Optional[str] = None,
    model: Optional[str] = None,
    max_steps: Optional[int] = None,
    execution_mode: str = "single",  # "single", "batch", "interactive"
    run_id: Optional[str] = None,
) -> None:
    """Track task execution start.

    Args:
        env_name: Name of the environment/task
        task_index: Task variant index
        provider_type: Provider type (simulated, webtop, native, computer)
        os_type: OS type (linux, windows, android)
        agent: Agent name if specified
        model: Model name if specified
        max_steps: Max steps budget
        execution_mode: Execution mode (single, batch, interactive)
        run_id: Run ID for correlation
    """
    properties = {
        "env_name": env_name,
        "task_index": task_index,
        "execution_mode": execution_mode,
    }
    if provider_type:
        properties["provider_type"] = provider_type
    if os_type:
        properties["os_type"] = os_type
    if agent:
        properties["agent"] = agent
    if model:
        properties["model"] = model
    if max_steps:
        properties["max_steps"] = max_steps
    if run_id:
        properties["run_id"] = run_id

    record_event("cb_task_execution_started", properties)


def track_task_evaluation_completed(
    env_name: str,
    task_index: int,
    *,
    result: Any,
    success: bool,
    total_steps: int,
    duration_seconds: float,
    run_id: Optional[str] = None,
    agent: Optional[str] = None,
    model: Optional[str] = None,
) -> None:
    """Track task evaluation completion.

    Args:
        env_name: Name of the environment/task
        task_index: Task variant index
        result: Evaluation result (reward/score)
        success: Whether task was successful
        total_steps: Total steps taken
        duration_seconds: Total duration in seconds
        run_id: Run ID for correlation
        agent: Agent name if used
        model: Model name if used
    """
    properties = {
        "env_name": env_name,
        "task_index": task_index,
        "success": success,
        "total_steps": total_steps,
        "duration_seconds": round(duration_seconds, 2),
    }

    # Handle different result types
    if isinstance(result, (int, float)):
        properties["reward"] = float(result)
    elif result is not None:
        properties["result_type"] = type(result).__name__

    if run_id:
        properties["run_id"] = run_id
    if agent:
        properties["agent"] = agent
    if model:
        properties["model"] = model

    record_event("cb_task_evaluation_completed", properties)


def track_batch_job_started(
    dataset_name: str,
    task_count: int,
    variant_count: int,
    *,
    parallelism: int = 1,
    agent: Optional[str] = None,
    model: Optional[str] = None,
    run_id: Optional[str] = None,
    provider_type: Optional[str] = None,
) -> None:
    """Track batch job start.

    Args:
        dataset_name: Name of the dataset
        task_count: Number of unique tasks
        variant_count: Total variants to run
        parallelism: Max parallel workers
        agent: Agent name if specified
        model: Model name if specified
        run_id: Run ID for correlation
        provider_type: Provider type
    """
    properties = {
        "dataset_name": dataset_name,
        "task_count": task_count,
        "variant_count": variant_count,
        "parallelism": parallelism,
    }
    if agent:
        properties["agent"] = agent
    if model:
        properties["model"] = model
    if run_id:
        properties["run_id"] = run_id
    if provider_type:
        properties["provider_type"] = provider_type

    record_event("cb_batch_job_started", properties)


# =============================================================================
# Tier 2 Events (High Value - Usage Patterns)
# =============================================================================


def track_task_step_executed(
    action_type: str,
    step_count: int,
    *,
    duration_ms: Optional[float] = None,
    run_id: Optional[str] = None,
) -> None:
    """Track individual step execution.

    Note: This should be sampled to avoid high event volume.

    Args:
        action_type: Type of action (ClickAction, TypeAction, etc.)
        step_count: Current step number
        duration_ms: Step duration in milliseconds
        run_id: Run ID for correlation
    """
    properties = {
        "action_type": action_type,
        "step_count": step_count,
    }
    if duration_ms is not None:
        properties["duration_ms"] = round(duration_ms, 2)
    if run_id:
        properties["run_id"] = run_id

    record_event("cb_task_step_executed", properties)


def track_batch_task_completed(
    env_name: str,
    task_index: int,
    *,
    success: bool,
    reward: Optional[float] = None,
    total_steps: int = 0,
    duration_seconds: float = 0,
    run_id: Optional[str] = None,
    error: Optional[str] = None,
) -> None:
    """Track individual task completion in batch.

    Args:
        env_name: Name of the environment/task
        task_index: Task variant index
        success: Whether task succeeded
        reward: Reward/score if available
        total_steps: Steps taken
        duration_seconds: Task duration
        run_id: Run ID for correlation
        error: Error message if failed
    """
    properties = {
        "env_name": env_name,
        "task_index": task_index,
        "success": success,
        "total_steps": total_steps,
        "duration_seconds": round(duration_seconds, 2),
    }
    if reward is not None:
        properties["reward"] = float(reward)
    if run_id:
        properties["run_id"] = run_id
    if error:
        # Truncate error message
        properties["error_type"] = error.split(":")[0][:50] if ":" in error else error[:50]

    record_event("cb_batch_task_completed", properties)


def track_dataset_processing_completed(
    processor_mode: str,
    rows_processed: int,
    *,
    duration_seconds: float,
    success: bool = True,
    output_format: Optional[str] = None,
) -> None:
    """Track dataset processing completion.

    Args:
        processor_mode: Processing mode (aguvis-stage-1, gui-r1, etc.)
        rows_processed: Number of rows processed
        duration_seconds: Processing duration
        success: Whether processing succeeded
        output_format: Output format (disk, hub, jsonl)
    """
    properties = {
        "processor_mode": processor_mode,
        "rows_processed": rows_processed,
        "duration_seconds": round(duration_seconds, 2),
        "success": success,
    }
    if output_format:
        properties["output_format"] = output_format

    record_event("cb_dataset_processing_completed", properties)


def track_task_execution_failed(
    env_name: str,
    task_index: int,
    *,
    error_type: str,
    error_message: str,
    stage: str,  # "setup", "step", "solve", "evaluate"
    run_id: Optional[str] = None,
) -> None:
    """Track task execution failure.

    Args:
        env_name: Name of the environment/task
        task_index: Task variant index
        error_type: Exception class name
        error_message: Error message (truncated)
        stage: Stage where error occurred
        run_id: Run ID for correlation
    """
    properties = {
        "env_name": env_name,
        "task_index": task_index,
        "error_type": error_type[:100],
        "error_message": error_message[:200],  # Truncate for privacy
        "stage": stage,
    }
    if run_id:
        properties["run_id"] = run_id

    record_event("cb_task_execution_failed", properties)


# =============================================================================
# Decorators
# =============================================================================


def track_command(func: Callable) -> Callable:
    """Decorator to track command invocation.

    Usage:
        @track_command
        def cmd_run_task(args):
            ...
    """

    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        # Extract command name from function name
        command = func.__name__.replace("cmd_", "").replace("_", "-")

        # Try to extract subcommand and args from first argument
        subcommand = None
        cmd_args = {}
        if args:
            arg_obj = args[0]
            if hasattr(arg_obj, "run_command"):
                subcommand = getattr(arg_obj, "run_command", None)
            # Extract safe args
            for key in ["agent", "model", "max_steps", "platform", "oracle", "wait", "debug"]:
                if hasattr(arg_obj, key):
                    cmd_args[key] = getattr(arg_obj, key)

        track_command_invoked(command, subcommand, cmd_args)

        return func(*args, **kwargs)

    return wrapper


def track_command_async(func: Callable) -> Callable:
    """Async decorator to track command invocation."""

    @functools.wraps(func)
    async def wrapper(*args, **kwargs):
        command = func.__name__.replace("cmd_", "").replace("_async", "").replace("_", "-")

        subcommand = None
        cmd_args = {}
        if args:
            arg_obj = args[0]
            if hasattr(arg_obj, "run_command"):
                subcommand = getattr(arg_obj, "run_command", None)
            for key in ["agent", "model", "max_steps", "platform", "oracle", "wait", "debug"]:
                if hasattr(arg_obj, key):
                    cmd_args[key] = getattr(arg_obj, key)

        track_command_invoked(command, subcommand, cmd_args)

        return await func(*args, **kwargs)

    return wrapper
