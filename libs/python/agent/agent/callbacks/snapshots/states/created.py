"""
Created state - snapshot successfully created.
"""

from typing import Dict, Any
from .base import SnapshotState


class CreatedSnapshotState(SnapshotState):
    """
    State representing a successfully created snapshot.
    """

    def __init__(self, snapshot_info: Dict[str, Any]):
        """Initialize with snapshot information."""
        self.snapshot_info = snapshot_info

    def get_state_name(self) -> str:
        """Get the name of this state."""
        return "created"

    def can_create(self) -> bool:
        """Can create new snapshots when one is created."""
        return True

    def can_restore(self) -> bool:
        """Can restore from created snapshots."""
        return True

    def can_delete(self) -> bool:
        """Can delete created snapshots."""
        return True

    def handle_creation_started(self, context) -> 'SnapshotState':
        """Can start creating new snapshots."""
        from .creating import CreatingSnapshotState
        return CreatingSnapshotState()

    def handle_creation_completed(self, context, result: Dict[str, Any]) -> 'SnapshotState':
        """Update with new snapshot info."""
        return CreatedSnapshotState(result)

    def handle_creation_failed(self, context, error: str) -> 'SnapshotState':
        """Transition to failed state."""
        from .failed import FailedSnapshotState
        return FailedSnapshotState(error)

    def handle_restoration_started(self, context) -> 'SnapshotState':
        """Transition to restoring state."""
        from .restoring import RestoringSnapshotState
        return RestoringSnapshotState(self.snapshot_info)

    def handle_restoration_completed(self, context) -> 'SnapshotState':
        """Restoration completed, stay in created state."""
        return self

    def handle_restoration_failed(self, context, error: str) -> 'SnapshotState':
        """Restoration failed, stay in created state."""
        return self

    def get_status_info(self) -> Dict[str, Any]:
        """Get status information including snapshot details."""
        status = super().get_status_info()
        status["snapshot_info"] = self.snapshot_info
        return status