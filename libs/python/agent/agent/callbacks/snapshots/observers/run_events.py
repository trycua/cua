"""
Observers for run-related events.
"""

from typing import Dict, Any
import logging

from .base import SnapshotEventObserver

logger = logging.getLogger(__name__)


class RunStartObserver(SnapshotEventObserver):
    """
    Observer for run start events.
    No if-else statements - handles only run start.
    """

    def __init__(self, scheduling_strategy, command_invoker):
        """Initialize run start observer."""
        self.scheduling_strategy = scheduling_strategy
        self.command_invoker = command_invoker

    async def on_run_start(self, event_data: Dict[str, Any]) -> None:
        """Handle run start by checking strategy."""
        self.scheduling_strategy.start_new_run()

        if self.scheduling_strategy.should_create_on_run_start():
            await self._create_snapshot("run_start", event_data)

    async def on_run_end(self, event_data: Dict[str, Any]) -> None:
        """Not interested in run end events."""
        pass

    async def on_action_end(self, event_data: Dict[str, Any]) -> None:
        """Not interested in action end events."""
        pass

    def get_observer_name(self) -> str:
        """Get the name of this observer."""
        return "run_start_observer"

    def is_interested_in_event(self, event_type: str) -> bool:
        """Only interested in run start events."""
        return event_type == "run_start"

    async def _create_snapshot(self, trigger: str, event_data: Dict[str, Any]) -> None:
        """Create snapshot using command pattern."""
        from ..commands.create import CreateSnapshotCommand

        container_name = event_data.get("container_name")
        if not container_name:
            return

        command = CreateSnapshotCommand(
            event_data.get("snapshot_context"),
            event_data.get("snapshot_creator"),
            container_name,
            trigger,
            self.scheduling_strategy.get_run_context()
        )

        await self.command_invoker.execute_command(command)


class RunEndObserver(SnapshotEventObserver):
    """
    Observer for run end events.
    No if-else statements - handles only run end.
    """

    def __init__(self, scheduling_strategy, command_invoker, cleanup_policies):
        """Initialize run end observer."""
        self.scheduling_strategy = scheduling_strategy
        self.command_invoker = command_invoker
        self.cleanup_policies = cleanup_policies

    async def on_run_start(self, event_data: Dict[str, Any]) -> None:
        """Not interested in run start events."""
        pass

    async def on_run_end(self, event_data: Dict[str, Any]) -> None:
        """Handle run end by checking strategy and cleanup."""
        if self.scheduling_strategy.should_create_on_run_end():
            await self._create_snapshot("run_end", event_data)

        await self._perform_cleanup(event_data)

    async def on_action_end(self, event_data: Dict[str, Any]) -> None:
        """Not interested in action end events."""
        pass

    def get_observer_name(self) -> str:
        """Get the name of this observer."""
        return "run_end_observer"

    def is_interested_in_event(self, event_type: str) -> bool:
        """Only interested in run end events."""
        return event_type == "run_end"

    async def _create_snapshot(self, trigger: str, event_data: Dict[str, Any]) -> None:
        """Create snapshot using command pattern."""
        from ..commands.create import CreateSnapshotCommand

        container_name = event_data.get("container_name")
        if not container_name:
            return

        command = CreateSnapshotCommand(
            event_data.get("snapshot_context"),
            event_data.get("snapshot_creator"),
            container_name,
            trigger,
            self.scheduling_strategy.get_run_context()
        )

        await self.command_invoker.execute_command(command)

    async def _perform_cleanup(self, event_data: Dict[str, Any]) -> None:
        """Perform cleanup using polymorphic policies."""
        container_name = event_data.get("container_name")
        provider_adapter = event_data.get("provider_adapter")

        if not container_name or not provider_adapter:
            return

        for policy in self.cleanup_policies:
            await policy.cleanup(provider_adapter, container_name)