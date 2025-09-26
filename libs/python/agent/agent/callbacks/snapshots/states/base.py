"""
Base state interface for snapshot lifecycle.
"""

from abc import ABC, abstractmethod
from typing import Dict, Any, Optional


class SnapshotState(ABC):
    """
    Abstract base class for snapshot states.
    Eliminates if-else statements by using state pattern.
    """

    @abstractmethod
    def get_state_name(self) -> str:
        """Get the name of this state."""
        pass

    @abstractmethod
    def can_create(self) -> bool:
        """Check if snapshot can be created in this state."""
        pass

    @abstractmethod
    def can_restore(self) -> bool:
        """Check if snapshot can be restored in this state."""
        pass

    @abstractmethod
    def can_delete(self) -> bool:
        """Check if snapshot can be deleted in this state."""
        pass

    @abstractmethod
    def handle_creation_started(self, context) -> 'SnapshotState':
        """Handle transition when creation starts."""
        pass

    @abstractmethod
    def handle_creation_completed(self, context, result: Dict[str, Any]) -> 'SnapshotState':
        """Handle transition when creation completes."""
        pass

    @abstractmethod
    def handle_creation_failed(self, context, error: str) -> 'SnapshotState':
        """Handle transition when creation fails."""
        pass

    @abstractmethod
    def handle_restoration_started(self, context) -> 'SnapshotState':
        """Handle transition when restoration starts."""
        pass

    @abstractmethod
    def handle_restoration_completed(self, context) -> 'SnapshotState':
        """Handle transition when restoration completes."""
        pass

    @abstractmethod
    def handle_restoration_failed(self, context, error: str) -> 'SnapshotState':
        """Handle transition when restoration fails."""
        pass

    def get_status_info(self) -> Dict[str, Any]:
        """Get status information for this state."""
        return {
            "state": self.get_state_name(),
            "can_create": self.can_create(),
            "can_restore": self.can_restore(),
            "can_delete": self.can_delete()
        }