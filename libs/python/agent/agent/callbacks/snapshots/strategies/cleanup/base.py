"""
Base cleanup policy interface.
"""

from abc import ABC, abstractmethod
from typing import List, Dict, Any, Optional
from datetime import datetime


class CleanupPolicy(ABC):
    """
    Abstract base class for cleanup policies.
    Eliminates if-else statements by using polymorphism.
    """

    @abstractmethod
    async def should_cleanup(self, snapshots: List[Dict[str, Any]]) -> bool:
        """Determine if cleanup should be performed."""
        pass

    @abstractmethod
    async def get_snapshots_to_cleanup(self, snapshots: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Get list of snapshots that should be cleaned up."""
        pass

    @abstractmethod
    async def cleanup(self, provider_adapter: Any, container_name: str) -> List[Dict[str, Any]]:
        """Perform the cleanup operation."""
        pass

    @abstractmethod
    def get_policy_name(self) -> str:
        """Get the name of this cleanup policy."""
        pass

    def should_cleanup_immediately(self, last_cleanup_time: Optional[datetime] = None) -> bool:
        """Check if cleanup should be performed immediately."""
        return last_cleanup_time is None