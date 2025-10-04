"""
Observer for action-related events.
"""

from typing import Dict, Any
import logging

from .base import SnapshotEventObserver

logger = logging.getLogger(__name__)


class ActionEndObserver(SnapshotEventObserver):
    """
    Observer for action end events.
    No if-else statements - handles only action end.
    """

    def __init__(self, scheduling_strategy, command_invoker, trigger_descriptor):
        """Initialize action end observer."""
        self.scheduling_strategy = scheduling_strategy
        self.command_invoker = command_invoker
        self.trigger_descriptor = trigger_descriptor

    async def on_run_start(self, event_data: Dict[str, Any]) -> None:
        """Not interested in run start events."""
        pass

    async def on_run_end(self, event_data: Dict[str, Any]) -> None:
        """Not interested in run end events."""
        pass

    async def on_action_end(self, event_data: Dict[str, Any]) -> None:
        """Handle action end by checking strategy."""
        self.scheduling_strategy.increment_action_count()

        if self.scheduling_strategy.should_create_on_action():
            await self._create_snapshot(event_data)

    def get_observer_name(self) -> str:
        """Get the name of this observer."""
        return "action_end_observer"

    def is_interested_in_event(self, event_type: str) -> bool:
        """Only interested in action end events."""
        return event_type == "action_end"

    async def _create_snapshot(self, event_data: Dict[str, Any]) -> None:
        """Create snapshot using command pattern."""
        from ..commands.create import CreateSnapshotCommand

        container_name = event_data.get("container_name")
        action_details = event_data.get("action_details", {})

        if not container_name:
            return

        trigger = self.trigger_descriptor.get_description(
            "action",
            action_details,
            self.scheduling_strategy.action_count
        )

        command = CreateSnapshotCommand(
            event_data.get("snapshot_context"),
            event_data.get("snapshot_creator"),
            container_name,
            trigger,
            self.scheduling_strategy.get_run_context()
        )

        await self.command_invoker.execute_command(command)