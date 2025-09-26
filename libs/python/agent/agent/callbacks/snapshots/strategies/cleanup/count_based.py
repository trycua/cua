"""
Count-based cleanup policy.
"""

from typing import List, Dict, Any
import logging

from .base import CleanupPolicy

logger = logging.getLogger(__name__)


class CountBasedCleanupPolicy(CleanupPolicy):
    """
    Cleanup policy based on snapshot count limit.
    No if-else statements - uses polymorphic behavior.
    """

    def __init__(self, max_snapshots: int = 10, enabled: bool = True):
        """Initialize count-based cleanup policy."""
        self.max_snapshots = max_snapshots
        self.enabled = enabled

    async def should_cleanup(self, snapshots: List[Dict[str, Any]]) -> bool:
        """Check if snapshot count exceeds limit."""
        if not self.enabled:
            return False

        return len(snapshots) > self.max_snapshots

    async def get_snapshots_to_cleanup(self, snapshots: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Get oldest snapshots that exceed the limit."""
        if not self.enabled or len(snapshots) <= self.max_snapshots:
            return []

        sorted_snapshots = self._sort_snapshots_by_age(snapshots)
        excess_count = len(snapshots) - self.max_snapshots
        return sorted_snapshots[:excess_count]

    async def cleanup(self, provider_adapter: Any, container_name: str) -> List[Dict[str, Any]]:
        """Remove oldest snapshots that exceed the limit."""
        if not self.enabled:
            return []

        deleted_snapshots = []
        snapshots = await provider_adapter.list_snapshots(container_name)

        if len(snapshots) <= self.max_snapshots:
            return []

        logger.info(f"Enforcing snapshot limit: {len(snapshots)} > {self.max_snapshots}")

        snapshots_to_delete = await self.get_snapshots_to_cleanup(snapshots)

        for snapshot in snapshots_to_delete:
            snapshot_id = snapshot.get("id")
            snapshot_tag = snapshot.get("tag", "unknown")

            logger.info(f"Deleting snapshot over limit: {snapshot_tag} (ID: {snapshot_id[:8]})")

            result = await provider_adapter.delete_snapshot(snapshot_id)
            if result.get("status") == "deleted":
                deleted_snapshots.append(snapshot)

        return deleted_snapshots

    def get_policy_name(self) -> str:
        """Get the name of this cleanup policy."""
        return f"count_based_{self.max_snapshots}"

    def _sort_snapshots_by_age(self, snapshots: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Sort snapshots by timestamp, oldest first."""
        return sorted(snapshots, key=lambda x: x.get("timestamp", ""))