"""
Telemetry callback handler for Computer-Use Agent (cua-agent)
"""

import logging
import platform
import sys
import time
import uuid
from typing import Any, Dict, List, Optional, Union

from cua_core.telemetry import (
    is_telemetry_enabled,
    record_event,
)
from cua_core.telemetry._config import sanitize_model_name

from .base import AsyncCallbackHandler

logger = logging.getLogger(__name__)

# Coarse, non-identifying system info. No kernel release, hostname or paths.
SYSTEM_INFO = {
    "os": platform.system().lower(),
    "python_version": f"{sys.version_info.major}.{sys.version_info.minor}",
}

# Only these numeric usage keys are ever sent in agent_usage.
USAGE_KEYS = ("prompt_tokens", "completion_tokens", "total_tokens", "response_cost")

_trajectory_warning_emitted = False


def _warn_trajectory_sharing_moved() -> None:
    global _trajectory_warning_emitted
    if _trajectory_warning_emitted:
        return
    _trajectory_warning_emitted = True
    logger.warning(
        "log_trajectory=True no longer uploads anything: trajectory sharing is not "
        "available. Only anonymous usage telemetry is sent."
    )


def _coarse_agent_type(agent_loop: Any) -> str:
    """Built-in loop class name, or "custom" for user-registered loops."""
    cls = type(agent_loop)
    if (cls.__module__ or "").startswith("cua_agent."):
        return cls.__name__
    return "custom"


def _numeric(value: Any) -> Optional[Union[int, float]]:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return value
    return None


class TelemetryCallback(AsyncCallbackHandler):
    """
    Telemetry callback handler for Computer-Use Agent (cua-agent)

    Tracks anonymous agent usage and performance metrics. Never sends prompts,
    outputs, screenshots or other trajectory content.
    """

    def __init__(self, agent, log_trajectory: bool = False):
        """
        Initialize telemetry callback.

        Args:
            agent: The ComputerAgent instance
            log_trajectory: Deprecated. Trajectories are never uploaded through
                telemetry; setting this only logs a one-time warning.
        """
        self.agent = agent
        self.log_trajectory = log_trajectory
        if log_trajectory:
            _warn_trajectory_sharing_moved()

        # Generate session/run IDs
        self.session_id = str(uuid.uuid4())
        self.run_id = None

        # Track timing and metrics
        self.run_start_time = None
        self.step_count = 0
        self.step_start_time = None
        self.total_usage = {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
            "response_cost": 0.0,
        }

        # Record agent initialization
        if is_telemetry_enabled():
            self._record_agent_initialization()

    def _record_agent_initialization(self) -> None:
        """Record agent type/model and session initialization."""
        # Get the agent loop type (class name)
        agent_type = "unknown"
        if hasattr(self.agent, "agent_loop") and self.agent.agent_loop is not None:
            agent_type = _coarse_agent_type(self.agent.agent_loop)

        model = getattr(self.agent, "model", None)
        agent_info = {
            "session_id": self.session_id,
            "agent_type": agent_type,
            "model": (sanitize_model_name(model) if isinstance(model, str) else None) or "unknown",
            **SYSTEM_INFO,
        }

        record_event("agent_session_start", agent_info)

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Called at the start of an agent run loop."""
        if not is_telemetry_enabled():
            return

        self.run_id = str(uuid.uuid4())
        self.run_start_time = time.time()
        self.step_count = 0

        # Calculate input context size
        input_context_size = self._calculate_context_size(old_items)

        run_data = {
            "session_id": self.session_id,
            "run_id": self.run_id,
            "start_time": self.run_start_time,
            "input_context_size": input_context_size,
            "num_existing_messages": len(old_items),
        }

        record_event("agent_run_start", run_data)

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        """Called at the end of an agent run loop."""
        if not is_telemetry_enabled() or not self.run_start_time:
            return

        run_duration = time.time() - self.run_start_time

        run_data = {
            "session_id": self.session_id,
            "run_id": self.run_id,
            "end_time": time.time(),
            "duration_seconds": run_duration,
            "num_steps": self.step_count,
            "total_usage": self.total_usage.copy(),
        }

        record_event("agent_run_end", run_data)

    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """Called when usage information is received."""
        if not is_telemetry_enabled():
            return

        # Only known numeric keys; provider-specific extras are never sent.
        known: Dict[str, Union[int, float]] = {}
        for key in USAGE_KEYS:
            value = _numeric(usage.get(key))
            if value is not None:
                known[key] = value
                self.total_usage[key] += value

        usage_data = {
            "session_id": self.session_id,
            "run_id": self.run_id,
            "step": self.step_count,
            **known,
        }

        record_event("agent_usage", usage_data)

    async def on_responses(self, kwargs: Dict[str, Any], responses: Dict[str, Any]) -> None:
        """Called when responses are received."""
        if not is_telemetry_enabled():
            return

        self.step_count += 1
        step_duration = None

        if self.step_start_time:
            step_duration = time.time() - self.step_start_time

        self.step_start_time = time.time()

        step_data = {
            "session_id": self.session_id,
            "run_id": self.run_id,
            "step": self.step_count,
            "timestamp": self.step_start_time,
        }

        if step_duration is not None:
            step_data["duration_seconds"] = step_duration

        record_event("agent_step", step_data)

    def _calculate_context_size(self, items: List[Dict[str, Any]]) -> int:
        """Calculate approximate context size in tokens/characters."""
        total_size = 0

        for item in items:
            if item.get("type") == "message" and "content" in item:
                content = item["content"]
                if isinstance(content, str):
                    total_size += len(content)
                elif isinstance(content, list):
                    for part in content:
                        if isinstance(part, dict) and "text" in part:
                            total_size += len(part["text"])
            elif "content" in item and isinstance(item["content"], str):
                total_size += len(item["content"])

        return total_size
