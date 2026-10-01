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
import platform
import sys
from typing import Any, Callable, Dict, Optional

try:  # cua-core >= 0.3 (import name `cua_core`); optional so that cua-bench
    # co-installs with harnesses pinned to cua-core 0.1 (import name `core`).
    from cua_core.telemetry import is_telemetry_enabled as _core_is_telemetry_enabled
    from cua_core.telemetry import record_event as _core_record_event
    from cua_core.telemetry.posthog import PostHogTelemetryClient
except ImportError:  # pragma: no cover - telemetry is off without cua-core

    def _core_is_telemetry_enabled() -> bool:
        return False

    def _core_record_event(event_name: str, properties: Optional[Dict[str, Any]] = None) -> None:
        return None

    PostHogTelemetryClient = None  # type: ignore[assignment,misc]

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
    """Coarse properties attached to every cua-bench event.

    No kernel release, hostname, paths or client timestamps.
    """
    return {
        "bench_version": _get_version(),
        "python_version": f"{sys.version_info.major}.{sys.version_info.minor}",
        "os": platform.system().lower(),
    }


# =============================================================================
# Sanitizers: everything user-chosen is mapped to a fixed vocabulary.
# =============================================================================

#: Dataset (task set) names shipped in ``cua_bench/registry.json``.
KNOWN_TASKSETS = frozenset({"cua-bench-basic", "cua-bench-kicad", "cua-bench-workflows"})

#: Built-in task/environment names found in this repository (datasets/,
#: tasks/, example_tasks/). Anything else is reported as ``"custom"`` because
#: a user's task directory name can be personal.
KNOWN_ENV_NAMES = frozenset(
    {
        *KNOWN_TASKSETS,
        # datasets/cua-bench-basic
        "click-button",
        "click-icon",
        "color-picker",
        "date-picker",
        "drag-drop",
        "drag-slider",
        "fill-form",
        "right-click-menu",
        "select-dropdown",
        "spreadsheet-cell",
        "toggle-switch",
        "typing-input",
        "video-player",
        # datasets/cua-bench-workflows
        "openshot-tasks",
        "unity-tasks",
        # datasets/cua-bench-kicad
        "154d0750",
        "1625e97a",
        "2c11fe94",
        "34877be3",
        "458f5f5c",
        "4bd3e530",
        "4c913e30",
        "55f2eefb",
        "589da233",
        "5dbc9e02",
        "6cb23037",
        "733d278a",
        "83490c3a",
        "84ec333e",
        "854b8a80",
        "9ca3ef44",
        "a6414tw4",
        "a6dd38a9",
        "b7148878",
        "b7e51b92",
        "ba34f1c3",
        "c4baee36",
        "d1c655da",
        "d5947ae7",
        "ff6dfdde",
        # tasks/
        "miniwob",
        "online_mind2web",
        "osworld",
        "osworld_g",
        "screenspot_pro",
        "webgym",
        "webvoyager",
        "winarena_adapter",
        "wordpad_env",
        # example_tasks/
        "2048_env",
        "advanced_gui_env",
        "basic_gui_env",
        "hello_file_env",
        "minesweeper_game_env",
        "test_click_env",
        "test_notepad_env",
    }
)

#: Agents registered by cua-bench itself (``@register_agent``).
KNOWN_AGENTS = frozenset({"cua-agent", "gemini", "opencua", "qwen3vl", "qwen35", "harness"})
KNOWN_PROVIDERS = frozenset({"native", "computer", "simulated", "webtop"})
KNOWN_OS_TYPES = frozenset({"linux", "windows", "macos", "android"})

#: CLI arguments that may be reported, each with its sanitizer.
_SAFE_ARG_KEYS = (
    "agent",
    "model",
    "max_steps",
    "platform",
    "provider_type",
    "max_parallel",
    "oracle",
    "wait",
    "debug",
    "on",
    "kind",
    "runtime",
    "detach",
)
_ON_VALUES = frozenset({"local", "cloud"})
_KIND_VALUES = frozenset({"auto", "container", "vm"})
_RUNTIME_VALUES = frozenset({"auto", "gvisor", "runc", "qemu", "lume", "kubevirt"})


def _strip_version(name: str) -> str:
    return name.split("@", 1)[0]


def _basename(name: str) -> str:
    text = str(name).strip().rstrip("/\\")
    for sep in ("/", "\\"):
        text = text.rsplit(sep, 1)[-1]
    return text


def taskset_id(name: Optional[str]) -> str:
    """Known cua-bench task set name, or ``"custom"``."""
    if not name:
        return "custom"
    base = _strip_version(_basename(str(name)))
    return base if base in KNOWN_TASKSETS else "custom"


def env_name_label(name: Optional[str]) -> str:
    """Known built-in task/environment name, or ``"custom"``."""
    if not name:
        return "custom"
    base = _basename(str(name))
    return base if base in KNOWN_ENV_NAMES else "custom"


def agent_label(agent: Optional[str]) -> Optional[str]:
    if agent is None:
        return None
    return agent if agent in KNOWN_AGENTS else "custom"


try:  # shared sanitizer (cua-core >= 0.3)
    from cua_core.telemetry._config import sanitize_model_name as _sanitize_model_name
except ImportError:  # pragma: no cover - older cua-core

    def _sanitize_model_name(model: Optional[str], max_len: int = 64) -> Optional[str]:
        if model is None:
            return None
        text = str(model)
        if len(text) > max_len or "/" in text or "\\" in text or ":" in text or " " in text:
            return "custom"
        return text


def model_label(model: Optional[str]) -> Optional[str]:
    """Coarse model id: path- or URL-like, unknown-org or long ids -> ``"custom"``."""
    if model is None:
        return None
    return _sanitize_model_name(str(model)) or "custom"


def _choice(value: Any, allowed: frozenset) -> str:
    return value if isinstance(value, str) and value in allowed else "other"


def score_bucket(rate: Optional[float]) -> str:
    """Aggregate success rate (0..1) as a coarse percent bucket."""
    if rate is None:
        return "none"
    try:
        pct = float(rate) * 100.0
    except (TypeError, ValueError):
        return "none"
    if pct != pct:  # NaN
        return "none"
    if pct <= 0:
        return "0"
    if pct >= 100:
        return "100"
    if pct < 25:
        return "1_24"
    if pct < 50:
        return "25_49"
    if pct < 75:
        return "50_74"
    return "75_99"


def count_bucket(n: Optional[int]) -> str:
    """Task count as a coarse bucket."""
    try:
        n = int(n or 0)
    except (TypeError, ValueError):
        return "0"
    if n <= 0:
        return "0"
    if n == 1:
        return "1"
    if n < 5:
        return "2_4"
    if n < 10:
        return "5_9"
    if n < 50:
        return "10_49"
    if n < 100:
        return "50_99"
    return "gte_100"


def _sanitize_args(args: Dict[str, Any]) -> Dict[str, Any]:
    out: Dict[str, Any] = {}
    for key in _SAFE_ARG_KEYS:
        value = args.get(key)
        if value is None:
            continue
        if key == "agent":
            out[key] = agent_label(str(value))
        elif key == "model":
            out[key] = model_label(str(value))
        elif key == "on":
            out[key] = _choice(value, _ON_VALUES)
        elif key == "kind":
            out[key] = _choice(value, _KIND_VALUES)
        elif key == "runtime":
            out[key] = _choice(value, _RUNTIME_VALUES)
        elif key in ("provider_type", "platform"):
            out[key] = _choice(value, KNOWN_PROVIDERS | KNOWN_OS_TYPES)
        elif isinstance(value, bool):
            out[key] = value
        elif isinstance(value, int):
            out[key] = value
    return out


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
        logger.debug(f"Failed to record event {event_name}: {type(e).__name__}")


def flush_telemetry() -> None:
    """Flush pending telemetry events.

    Delegates to cua-core's PostHog client.
    """
    if PostHogTelemetryClient is None or not is_telemetry_enabled():
        return
    try:
        client = PostHogTelemetryClient.get_client()
        client.flush()
    except Exception as e:
        logger.debug(f"Failed to flush telemetry: {type(e).__name__}")


def _error_class(text: Optional[str]) -> str:
    """Exception class name only (identifier-like prefix), else ``"other"``."""
    if not text:
        return "other"
    head = str(text).split(":", 1)[0].strip()
    head = head.rsplit(".", 1)[-1]
    return head if head.isidentifier() and len(head) <= 64 else "other"


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
    properties: Dict[str, Any] = {"command": command}
    if subcommand:
        properties["subcommand"] = subcommand
    if args:
        properties["args"] = _sanitize_args(args)

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
    properties: Dict[str, Any] = {
        "env_name": env_name_label(env_name),
        "task_index": task_index,
        "execution_mode": _choice(execution_mode, frozenset({"single", "batch", "interactive"})),
    }
    if provider_type:
        properties["provider_type"] = _choice(provider_type, KNOWN_PROVIDERS)
    if os_type:
        properties["os_type"] = _choice(os_type, KNOWN_OS_TYPES)
    if agent:
        properties["agent"] = agent_label(agent)
    if model:
        properties["model"] = model_label(model)
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
    properties: Dict[str, Any] = {
        "env_name": env_name_label(env_name),
        "task_index": task_index,
        "success": bool(success),
        "total_steps": total_steps,
        "duration_seconds": round(duration_seconds, 2),
    }

    # Only a numeric reward; never the result object itself.
    if isinstance(result, (int, float)) and not isinstance(result, bool):
        properties["reward"] = float(result)
    elif result is not None:
        properties["result_type"] = _choice(
            type(result).__name__, frozenset({"bool", "dict", "list", "tuple", "str", "NoneType"})
        )

    if run_id:
        properties["run_id"] = run_id
    if agent:
        properties["agent"] = agent_label(agent)
    if model:
        properties["model"] = model_label(model)

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
    properties: Dict[str, Any] = {
        "dataset_name": taskset_id(dataset_name),
        "task_count": task_count,
        "variant_count": variant_count,
        "parallelism": parallelism,
    }
    if agent:
        properties["agent"] = agent_label(agent)
    if model:
        properties["model"] = model_label(model)
    if run_id:
        properties["run_id"] = run_id
    if provider_type:
        properties["provider_type"] = _choice(provider_type, KNOWN_PROVIDERS)

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
    properties: Dict[str, Any] = {
        "action_type": (
            action_type if action_type.isidentifier() and len(action_type) <= 40 else "other"
        ),
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
        error: Error text if failed. Only an exception class name at its start
            (``"TimeoutError: ..."`` -> ``"TimeoutError"``) is ever sent.
    """
    properties: Dict[str, Any] = {
        "env_name": env_name_label(env_name),
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
        properties["error_type"] = _error_class(error)

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
    stage: str,  # "setup", "step", "solve", "evaluate"
    run_id: Optional[str] = None,
    error_message: Optional[str] = None,
) -> None:
    """Track task execution failure.

    Args:
        env_name: Name of the environment/task
        task_index: Task variant index
        error_type: Exception class name
        stage: Stage where error occurred
        run_id: Run ID for correlation
        error_message: Ignored. Accepted for backwards compatibility; raw
            exception messages are never sent.
    """
    del error_message
    properties: Dict[str, Any] = {
        "env_name": env_name_label(env_name),
        "task_index": task_index,
        "error_type": _error_class(error_type),
        "stage": _choice(stage, frozenset({"setup", "step", "solve", "evaluate"})),
    }
    if run_id:
        properties["run_id"] = run_id

    record_event("cb_task_execution_failed", properties)


_OUTCOMES = frozenset({"ok", "error", "cancelled"})


def track_bench_run_completed(
    taskset: Optional[str],
    score: Optional[float],
    task_count: Optional[int],
    outcome: str,
) -> None:
    """Fire ``cua_bench_run_completed`` once when a run/batch finishes.

    Properties are exactly ``taskset`` (known set or ``"custom"``),
    ``score_bucket``, ``task_count`` (bucket) and ``outcome``. Sent through the
    cua SDK when it exposes ``telemetry_record_bench_run``; otherwise through
    cua-core. Never raises.
    """
    if not is_telemetry_enabled():
        return
    ts = taskset_id(taskset)
    outcome = outcome if outcome in _OUTCOMES else "error"
    rate: Optional[float] = None
    if score is not None:
        try:
            rate = min(max(float(score), 0.0), 1.0)
            if rate != rate:
                rate = None
        except (TypeError, ValueError):
            rate = None
    try:
        count = max(int(task_count or 0), 0)
    except (TypeError, ValueError):
        count = 0

    try:
        import cua._native as _n  # type: ignore[import-not-found]

        fn = getattr(_n, "telemetry_record_bench_run", None)
        if fn is not None:
            fn(ts, rate, count, outcome)
            return
    except Exception as e:  # SDK missing or failed: fall back to cua-core
        logger.debug(f"cua SDK telemetry unavailable: {type(e).__name__}")

    try:
        _core_record_event(
            "cua_bench_run_completed",
            {
                "taskset": ts,
                "score_bucket": score_bucket(rate),
                "task_count": count_bucket(count),
                "outcome": outcome,
            },
        )
    except Exception as e:
        logger.debug(f"Failed to record cua_bench_run_completed: {type(e).__name__}")


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
