"""
Creating state - snapshot creation in progress.
"""

from typing import Dict, Any
from .base import SnapshotState


class CreatingSnapshotState(SnapshotState):
    """
    State representing a snapshot being created.
    """

    def get_state_name(self) -> str:
        """Get the name of this state."""
        return "creating"

    def can_create(self) -> bool:
        """Cannot create another snapshot while creating."""
        return False

    def can_restore(self) -> bool:
        """Cannot restore while creating."""
        return False

    def can_delete(self) -> bool:
        """Cannot delete while creating."""
        return False

    def handle_creation_started(self, context) -> 'SnapshotState':
        """Already in creating state."""
        return self

    def handle_creation_completed(self, context, result: Dict[str, Any]) -> 'SnapshotState':
        """Transition to created state."""
        from .created import CreatedSnapshotState
        return CreatedSnapshotState(result)

    def handle_creation_failed(self, context, error: str) -> 'SnapshotState':
        """Transition to failed state."""
        from .failed import FailedSnapshotState
        return FailedSnapshotState(error)

    def handle_restoration_started(self, context) -> 'SnapshotState':
        """Cannot restore while creating."""
        raise ValueError("Cannot restore while creating snapshot")

    def handle_restoration_completed(self, context) -> 'SnapshotState':
        """Cannot restore while creating."""
        raise ValueError("Cannot restore while creating snapshot")

    def handle_restoration_failed(self, context, error: str) -> 'SnapshotState':
        """Cannot restore while creating."""
        raise ValueError("Cannot restore while creating snapshot")