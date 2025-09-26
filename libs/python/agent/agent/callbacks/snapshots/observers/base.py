"""
Base observer interface for snapshot events.
"""

from abc import ABC, abstractmethod
from typing import Dict, Any, List


class SnapshotEventObserver(ABC):
    """
    Abstract base class for snapshot event observers.
    Eliminates if-else statements by using observer pattern.
    """

    @abstractmethod
    async def on_run_start(self, event_data: Dict[str, Any]) -> None:
        """Handle run start event."""
        pass

    @abstractmethod
    async def on_run_end(self, event_data: Dict[str, Any]) -> None:
        """Handle run end event."""
        pass

    @abstractmethod
    async def on_action_end(self, event_data: Dict[str, Any]) -> None:
        """Handle action end event."""
        pass

    @abstractmethod
    def get_observer_name(self) -> str:
        """Get the name of this observer."""
        pass

    def is_interested_in_event(self, event_type: str) -> bool:
        """Check if observer is interested in event type."""
        return True  # By default, interested in all events