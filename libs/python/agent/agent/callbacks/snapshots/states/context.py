"""
Context class for managing snapshot state transitions.
"""

from typing import Dict, Any
import logging

from .base import SnapshotState
from .pending import PendingSnapshotState

logger = logging.getLogger(__name__)


class SnapshotContext:
    """
    Context class that manages snapshot state transitions.
    Eliminates if-else statements by delegating to state objects.
    """

    def __init__(self, container_name: str):
        """Initialize with pending state."""
        self.container_name = container_name
        self.state: SnapshotState = PendingSnapshotState()

    def can_create(self) -> bool:
        """Check if snapshot can be created."""
        return self.state.can_create()

    def can_restore(self) -> bool:
        """Check if snapshot can be restored."""
        return self.state.can_restore()

    def can_delete(self) -> bool:
        """Check if snapshot can be deleted."""
        return self.state.can_delete()

    def start_creation(self) -> None:
        """Start snapshot creation."""
        logger.info(f"Starting snapshot creation for {self.container_name}")
        self.state = self.state.handle_creation_started(self)

    def complete_creation(self, result: Dict[str, Any]) -> None:
        """Complete snapshot creation."""
        logger.info(f"Snapshot creation completed for {self.container_name}")
        self.state = self.state.handle_creation_completed(self, result)

    def fail_creation(self, error: str) -> None:
        """Fail snapshot creation."""
        logger.error(f"Snapshot creation failed for {self.container_name}: {error}")
        self.state = self.state.handle_creation_failed(self, error)

    def start_restoration(self) -> None:
        """Start snapshot restoration."""
        logger.info(f"Starting snapshot restoration for {self.container_name}")
        self.state = self.state.handle_restoration_started(self)

    def complete_restoration(self) -> None:
        """Complete snapshot restoration."""
        logger.info(f"Snapshot restoration completed for {self.container_name}")
        self.state = self.state.handle_restoration_completed(self)

    def fail_restoration(self, error: str) -> None:
        """Fail snapshot restoration."""
        logger.error(f"Snapshot restoration failed for {self.container_name}: {error}")
        self.state = self.state.handle_restoration_failed(self, error)

    def get_state_name(self) -> str:
        """Get current state name."""
        return self.state.get_state_name()

    def get_status_info(self) -> Dict[str, Any]:
        """Get comprehensive status information."""
        status = self.state.get_status_info()
        status["container_name"] = self.container_name
        return status