"""
Core snapshot scheduling logic.
"""

from typing import Optional
import logging
from .run_context import RunContextManager
from .trigger_utils import TriggerDescriptor

# Import within methods to avoid circular imports

logger = logging.getLogger(__name__)


class SnapshotScheduler:
    """
    Manages snapshot scheduling based on configured intervals.
    """

    VALID_INTERVALS = [
        "manual",
        "every_action",
        "run_start",
        "run_end",
        "run_boundaries"
    ]

    def __init__(self, snapshot_interval: str = "manual"):
        """
        Initialize the snapshot scheduler.

        Args:
            snapshot_interval: When to create snapshots
        """

        self.snapshot_interval = self._validate_interval(snapshot_interval)
        self.run_context = RunContextManager()
        self.trigger_descriptor = TriggerDescriptor()

        logger.info(f"SnapshotScheduler initialized with interval: {self.snapshot_interval}")

    def should_create_snapshot_on_run_start(self) -> bool:
        """Determine if a snapshot should be created at run start."""
        return self.snapshot_interval in ["run_start", "run_boundaries"]

    def should_create_snapshot_on_run_end(self) -> bool:
        """Determine if a snapshot should be created at run end."""
        return self.snapshot_interval in ["run_end", "run_boundaries"]

    def should_create_snapshot_on_action(self) -> bool:
        """Determine if a snapshot should be created after an action."""
        return self.snapshot_interval == "every_action"

    def start_new_run(self) -> str:
        """Start tracking a new run."""
        return self.run_context.start_new_run()

    def increment_action_count(self) -> int:
        """Increment the action counter."""
        return self.run_context.increment_action_count()

    def get_trigger_description(self, event_type: str, action_details: Optional[dict] = None) -> str:
        """Generate a description of what triggered the snapshot."""
        return self.trigger_descriptor.get_description(
            event_type, action_details, self.run_context.action_count
        )

    def get_run_context(self) -> dict:
        """Get the current run context."""
        context = self.run_context.get_context()
        context["snapshot_interval"] = self.snapshot_interval
        return context

    def reset_run_context(self) -> None:
        """Reset the run context."""
        self.run_context.reset()

    def update_interval(self, new_interval: str) -> None:
        """Update the snapshot interval."""
        old_interval = self.snapshot_interval
        self.snapshot_interval = self._validate_interval(new_interval)

        if old_interval != self.snapshot_interval:
            logger.info(f"Snapshot interval updated from {old_interval} to {self.snapshot_interval}")

    def is_manual_mode(self) -> bool:
        """Check if scheduler is in manual mode."""
        return self.snapshot_interval == "manual"

    def get_statistics(self) -> dict:
        """Get scheduler statistics."""
        stats = self.run_context.get_statistics()
        stats["current_interval"] = self.snapshot_interval
        stats["is_manual"] = self.is_manual_mode()
        return stats

    def _validate_interval(self, interval: str) -> str:
        """Validate and normalize the snapshot interval."""
        if interval not in self.VALID_INTERVALS:
            logger.warning(f"Invalid snapshot interval '{interval}', defaulting to 'manual'")
            return "manual"
        return interval