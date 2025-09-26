"""
Pending state - snapshot not yet created.
"""

from typing import Dict, Any
from .base import SnapshotState


class PendingSnapshotState(SnapshotState):
    """
    State representing a snapshot that hasn't been created yet.
    """

    def get_state_name(self) -> str:
        """Get the name of this state."""
        return "pending"

    def can_create(self) -> bool:
        """Can create snapshots in pending state."""
        return True

    def can_restore(self) -> bool:
        """Cannot restore in pending state."""
        return False

    def can_delete(self) -> bool:
        """Cannot delete in pending state."""
        return False

    def handle_creation_started(self, context) -> 'SnapshotState':
        """Transition to creating state."""
        from .creating import CreatingSnapshotState
        return CreatingSnapshotState()

    def handle_creation_completed(self, context, result: Dict[str, Any]) -> 'SnapshotState':
        """Invalid transition from pending to completed."""
        raise ValueError("Cannot complete creation from pending state")

    def handle_creation_failed(self, context, error: str) -> 'SnapshotState':
        """Transition to failed state."""
        from .failed import FailedSnapshotState
        return FailedSnapshotState(error)

    def handle_restoration_started(self, context) -> 'SnapshotState':
        """Cannot restore from pending state."""
        raise ValueError("Cannot restore from pending state")

    def handle_restoration_completed(self, context) -> 'SnapshotState':
        """Cannot restore from pending state."""
        raise ValueError("Cannot restore from pending state")

    def handle_restoration_failed(self, context, error: str) -> 'SnapshotState':
        """Cannot restore from pending state."""
        raise ValueError("Cannot restore from pending state")