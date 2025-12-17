"""
OpenTelemetry callback handler for Computer-Use Agent (cua-agent).

Instruments agent operations for the Four Golden Signals:
- Latency: Operation duration
- Traffic: Operation counts
- Errors: Error counts
- Saturation: Concurrent operations
"""

import time
from typing import Any, Dict, List, Optional

from .base import AsyncCallbackHandler

# Import OTEL functions - these are available when cua-core[telemetry] is installed
try:
    from core.telemetry import (
        add_breadcrumb,
        capture_exception,
        create_span,
        is_otel_enabled,
        record_error,
        record_operation,
        record_tokens,
        set_context,
        track_concurrent,
    )

    OTEL_AVAILABLE = True
except ImportError:
    OTEL_AVAILABLE = False

    def is_otel_enabled() -> bool:
        return False


class OtelCallback(AsyncCallbackHandler):
    """
    OpenTelemetry callback handler for instrumentation.

    Tracks:
    - Agent session lifecycle (start/end)
    - Agent run lifecycle (start/end with duration)
    - Individual steps (with duration)
    - Computer actions (with duration)
    - Token usage
    - Errors
    """

    def __init__(self, agent: Any):
        """
        Initialize OTEL callback.

        Args:
            agent: The ComputerAgent instance
        """
        self.agent = agent
        self.model = getattr(agent, "model", "unknown")

        # Timing state
        self.run_start_time: Optional[float] = None
        self.step_start_time: Optional[float] = None
        self.step_count = 0

        # Span management
        self._session_span: Optional[Any] = None
        self._run_span: Optional[Any] = None

        # Track concurrent sessions
        self._concurrent_tracker: Optional[Any] = None

        if OTEL_AVAILABLE and is_otel_enabled():
            # Set context for all events
            set_context(
                "agent",
                {
                    "model": self.model,
                    "agent_type": self._get_agent_type(),
                },
            )

    def _get_agent_type(self) -> str:
        """Get the agent loop type name."""
        if hasattr(self.agent, "agent_loop") and self.agent.agent_loop is not None:
            return type(self.agent.agent_loop).__name__
        return "unknown"

    async def on_run_start(
        self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]
    ) -> None:
        """Called at the start of an agent run loop."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        self.run_start_time = time.perf_counter()
        self.step_count = 0

        # Add breadcrumb for debugging
        add_breadcrumb(
            category="agent",
            message=f"Agent run started with model {self.model}",
            level="info",
            data={
                "model": self.model,
                "agent_type": self._get_agent_type(),
                "input_messages": len(old_items),
            },
        )

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        """Called at the end of an agent run loop."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        if self.run_start_time is not None:
            duration = time.perf_counter() - self.run_start_time

            # Record run metrics
            record_operation(
                operation="agent.run",
                duration_seconds=duration,
                status="success",
                model=self.model,
                steps=self.step_count,
            )

            add_breadcrumb(
                category="agent",
                message=f"Agent run completed in {duration:.2f}s",
                level="info",
                data={
                    "duration_seconds": duration,
                    "steps": self.step_count,
                    "output_messages": len(new_items),
                },
            )

        self.run_start_time = None

    async def on_responses(
        self, kwargs: Dict[str, Any], responses: Dict[str, Any]
    ) -> None:
        """Called when responses are received (each step)."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        self.step_count += 1
        current_time = time.perf_counter()

        # Calculate step duration if we have a start time
        if self.step_start_time is not None:
            step_duration = current_time - self.step_start_time
            record_operation(
                operation="agent.step",
                duration_seconds=step_duration,
                status="success",
                model=self.model,
                step_number=self.step_count,
            )

        # Start timing next step
        self.step_start_time = current_time

        add_breadcrumb(
            category="agent",
            message=f"Agent step {self.step_count} completed",
            level="info",
            data={"step": self.step_count},
        )

    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """Called when usage information is received."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        prompt_tokens = usage.get("prompt_tokens", 0)
        completion_tokens = usage.get("completion_tokens", 0)

        if prompt_tokens > 0 or completion_tokens > 0:
            record_tokens(
                prompt_tokens=prompt_tokens,
                completion_tokens=completion_tokens,
                model=self.model,
            )

    async def on_computer_call_start(self, item: Dict[str, Any]) -> None:
        """Called when a computer call is about to start."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        action = item.get("action", {})
        action_type = action.get("type", "unknown")

        add_breadcrumb(
            category="computer",
            message=f"Computer action: {action_type}",
            level="info",
            data={"action_type": action_type},
        )

    async def on_computer_call_end(
        self, item: Dict[str, Any], result: List[Dict[str, Any]]
    ) -> None:
        """Called when a computer call has completed."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        action = item.get("action", {})
        action_type = action.get("type", "unknown")

        # Record computer action metric
        # Note: We don't have precise timing here, so we record with 0 duration
        # The actual timing should be done in the computer module
        record_operation(
            operation=f"computer.action.{action_type}",
            duration_seconds=0,  # Timing handled elsewhere
            status="success",
            model=self.model,
        )

    async def on_api_start(self, kwargs: Dict[str, Any]) -> None:
        """Called when an LLM API call is about to start."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        add_breadcrumb(
            category="llm",
            message="LLM API call started",
            level="info",
            data={"model": self.model},
        )

    async def on_api_end(self, kwargs: Dict[str, Any], result: Any) -> None:
        """Called when an LLM API call has completed."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        add_breadcrumb(
            category="llm",
            message="LLM API call completed",
            level="info",
        )


class OtelErrorCallback(AsyncCallbackHandler):
    """
    Callback that captures errors and sends them to Sentry/OTEL.

    Should be added early in the callback chain to catch all errors.
    """

    def __init__(self, agent: Any):
        """
        Initialize error callback.

        Args:
            agent: The ComputerAgent instance
        """
        self.agent = agent
        self.model = getattr(agent, "model", "unknown")

    async def on_error(self, error: Exception, context: Dict[str, Any]) -> None:
        """Called when an error occurs during agent execution."""
        if not OTEL_AVAILABLE or not is_otel_enabled():
            return

        error_type = type(error).__name__
        operation = context.get("operation", "unknown")

        # Record error metric
        record_error(
            error_type=error_type,
            operation=operation,
            model=self.model,
        )

        # Capture exception in Sentry
        capture_exception(
            error,
            context={
                "model": self.model,
                "operation": operation,
                **{k: v for k, v in context.items() if k != "operation"},
            },
        )
