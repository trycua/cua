"""
Failed state - snapshot operation failed.
"""

from typing import Dict, Any
from .base import SnapshotState


class FailedSnapshotState(SnapshotState):
    """
    State representing a failed snapshot operation.
    """

    def __init__(self, error_message: str):
        """Initialize with error message."""
        self.error_message = error_message

    def get_state_name(self) -> str:
        """Get the name of this state."""
        return "failed"

    def can_create(self) -> bool:
        """Can retry creating after failure."""
        return True

    def can_restore(self) -> bool:
        """Cannot restore failed snapshots."""
        return False

    def can_delete(self) -> bool:
        """Cannot delete failed snapshots."""
        return False

    def handle_creation_started(self, context) -> 'SnapshotState':
        """Retry creation from failed state."""
        from .creating import CreatingSnapshotState
        return CreatingSnapshotState()

    def handle_creation_completed(self, context, result: Dict[str, Any]) -> 'SnapshotState':
        """Successfully created after failure."""
        from .created import CreatedSnapshotState
        return CreatedSnapshotState(result)

    def handle_creation_failed(self, context, error: str) -> 'SnapshotState':
        """Update error message."""
        return FailedSnapshotState(error)

    def handle_restoration_started(self, context) -> 'SnapshotState':
        """Cannot restore from failed state."""
        raise ValueError("Cannot restore failed snapshot")

    def handle_restoration_completed(self, context) -> 'SnapshotState':
        """Cannot restore from failed state."""
        raise ValueError("Cannot restore failed snapshot")

    def handle_restoration_failed(self, context, error: str) -> 'SnapshotState':
        """Cannot restore from failed state."""
        raise ValueError("Cannot restore failed snapshot")

    def get_status_info(self) -> Dict[str, Any]:
        """Get status information including error details."""
        status = super().get_status_info()
        status["error_message"] = self.error_message
        return status