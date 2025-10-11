"""
Restoring state - snapshot restoration in progress.
"""

from typing import Dict, Any
from .base import SnapshotState


class RestoringSnapshotState(SnapshotState):
    """
    State representing a snapshot being restored.
    """

    def __init__(self, snapshot_info: Dict[str, Any]):
        """Initialize with snapshot information."""
        self.snapshot_info = snapshot_info

    def get_state_name(self) -> str:
        """Get the name of this state."""
        return "restoring"

    def can_create(self) -> bool:
        """Cannot create while restoring."""
        return False

    def can_restore(self) -> bool:
        """Already restoring."""
        return False

    def can_delete(self) -> bool:
        """Cannot delete while restoring."""
        return False

    def handle_creation_started(self, context) -> 'SnapshotState':
        """Cannot create while restoring."""
        raise ValueError("Cannot create snapshot while restoring")

    def handle_creation_completed(self, context, result: Dict[str, Any]) -> 'SnapshotState':
        """Cannot create while restoring."""
        raise ValueError("Cannot create snapshot while restoring")

    def handle_creation_failed(self, context, error: str) -> 'SnapshotState':
        """Cannot create while restoring."""
        raise ValueError("Cannot create snapshot while restoring")

    def handle_restoration_started(self, context) -> 'SnapshotState':
        """Already restoring."""
        return self

    def handle_restoration_completed(self, context) -> 'SnapshotState':
        """Restoration completed, back to created state."""
        from .created import CreatedSnapshotState
        return CreatedSnapshotState(self.snapshot_info)

    def handle_restoration_failed(self, context, error: str) -> 'SnapshotState':
        """Restoration failed, back to created state."""
        from .created import CreatedSnapshotState
        return CreatedSnapshotState(self.snapshot_info)

    def get_status_info(self) -> Dict[str, Any]:
        """Get status information including snapshot details."""
        status = super().get_status_info()
        status["snapshot_info"] = self.snapshot_info
        return status