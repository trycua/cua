"""
Telemetry callback handler for Computer-Use Agent (cua-agent)
"""

import platform
import time
import uuid
from typing import Any, Dict, List, Optional, Union

from core.telemetry import (
    is_telemetry_enabled,
    record_event,
)

from .base import AsyncCallbackHandler

SYSTEM_INFO = {
    "os": platform.system().lower(),
    "os_version": platform.release(),
    "python_version": platform.python_version(),
}


class TelemetryCallback(AsyncCallbackHandler):
    """
    Telemetry callback handler for Computer-Use Agent (cua-agent)

    Tracks agent usage, performance metrics, and optionally trajectory data.
    """

    def __init__(self, agent, log_trajectory: bool = False):
        """
        Initialize telemetry callback.

        Args:
            agent: The ComputerAgent instance
            log_trajectory: Whether to log full trajectory items (opt-in)
        """
        self.agent = agent
        self.log_trajectory = log_trajectory

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
        agent_info = {
            "session_id": self.session_id,
            "agent_type": (
                self.agent.agent_loop.__name__ if hasattr(self.agent, "agent_loop") else "unknown"
            ),
            "model": getattr(self.agent, "model", "unknown"),
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

        # Log trajectory if opted in
        if self.log_trajectory:
            trajectory = self._extract_trajectory(old_items)
            if trajectory:
                run_data["uploaded_trajectory"] = trajectory

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

        # Log trajectory if opted in
        if self.log_trajectory:
            trajectory = self._extract_trajectory(new_items)
            if trajectory:
                run_data["uploaded_trajectory"] = trajectory

        record_event("agent_run_end", run_data)

    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """Called when usage information is received."""
        if not is_telemetry_enabled():
            return

        # Accumulate usage stats
        self.total_usage["prompt_tokens"] += usage.get("prompt_tokens", 0)
        self.total_usage["completion_tokens"] += usage.get("completion_tokens", 0)
        self.total_usage["total_tokens"] += usage.get("total_tokens", 0)
        self.total_usage["response_cost"] += usage.get("response_cost", 0.0)

        # Record individual usage event
        usage_data = {
            "session_id": self.session_id,
            "run_id": self.run_id,
            "step": self.step_count,
            **usage,
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

    def _extract_trajectory(self, items: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Extract trajectory items that should be logged."""
        trajectory = []

        for item in items:
            # Include user messages, assistant messages, reasoning, computer calls, and computer outputs
            if (
                item.get("role") == "user"  # User inputs
                or (
                    item.get("type") == "message" and item.get("role") == "assistant"
                )  # Model outputs
                or item.get("type") == "reasoning"  # Reasoning traces
                or item.get("type") == "computer_call"  # Computer actions
                or item.get("type") == "computer_call_output"  # Computer outputs
            ):
                # Create a copy of the item with timestamp
                trajectory_item = item.copy()
                trajectory_item["logged_at"] = time.time()
                trajectory.append(trajectory_item)

        return trajectory
