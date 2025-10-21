"""
Factory for creating snapshot event observers without if-else statements.
"""

from typing import List

from ..observers.run_events import RunStartObserver, RunEndObserver
from ..observers.action_events import ActionEndObserver
from ..observers.base import SnapshotEventObserver


class ObserverFactory:
    """
    Factory for creating snapshot event observers.
    Uses composition instead of if-else statements.
    """

    @staticmethod
    def create_standard_observers(
        scheduling_strategy,
        command_invoker,
        trigger_descriptor,
        cleanup_policies
    ) -> List[SnapshotEventObserver]:
        """
        Create standard set of observers.
        No if-else statements - always creates all observer types.

        Args:
            scheduling_strategy: Strategy for scheduling snapshots
            command_invoker: Invoker for executing commands
            trigger_descriptor: Descriptor for generating triggers
            cleanup_policies: List of cleanup policies

        Returns:
            List of all standard observers
        """
        return [
            RunStartObserver(scheduling_strategy, command_invoker),
            RunEndObserver(scheduling_strategy, command_invoker, cleanup_policies),
            ActionEndObserver(scheduling_strategy, command_invoker, trigger_descriptor)
        ]

    @staticmethod
    def create_run_observers_only(
        scheduling_strategy,
        command_invoker,
        cleanup_policies
    ) -> List[SnapshotEventObserver]:
        """Create only run-related observers."""
        return [
            RunStartObserver(scheduling_strategy, command_invoker),
            RunEndObserver(scheduling_strategy, command_invoker, cleanup_policies)
        ]

    @staticmethod
    def create_action_observers_only(
        scheduling_strategy,
        command_invoker,
        trigger_descriptor
    ) -> List[SnapshotEventObserver]:
        """Create only action-related observers."""
        return [
            ActionEndObserver(scheduling_strategy, command_invoker, trigger_descriptor)
        ]

    @staticmethod
    def create_minimal_observers(
        scheduling_strategy,
        command_invoker
    ) -> List[SnapshotEventObserver]:
        """Create minimal set of observers (run start only)."""
        return [
            RunStartObserver(scheduling_strategy, command_invoker)
        ]