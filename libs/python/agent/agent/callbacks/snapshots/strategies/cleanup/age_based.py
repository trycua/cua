"""
Age-based cleanup policy.
"""

from datetime import datetime, timedelta
from typing import List, Dict, Any, Optional
import logging

from .base import CleanupPolicy

logger = logging.getLogger(__name__)


class AgeBasedCleanupPolicy(CleanupPolicy):
    """
    Cleanup policy based on snapshot age.
    No if-else statements - uses polymorphic behavior.
    """

    def __init__(self, retention_days: int = 7, enabled: bool = True):
        """Initialize age-based cleanup policy."""
        self.retention_days = retention_days
        self.enabled = enabled

    async def should_cleanup(self, snapshots: List[Dict[str, Any]]) -> bool:
        """Check if any snapshots exceed age limit."""
        if not self.enabled:
            return False

        expired_snapshots = await self.get_snapshots_to_cleanup(snapshots)
        return len(expired_snapshots) > 0

    async def get_snapshots_to_cleanup(self, snapshots: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Get snapshots that are too old."""
        if not self.enabled:
            return []

        cutoff_date = datetime.now() - timedelta(days=self.retention_days)
        expired_snapshots = []

        for snapshot in snapshots:
            if self._is_snapshot_expired(snapshot, cutoff_date):
                expired_snapshots.append(snapshot)

        return expired_snapshots

    async def cleanup(self, provider_adapter: Any, container_name: str) -> List[Dict[str, Any]]:
        """Remove snapshots that are too old."""
        if not self.enabled:
            return []

        deleted_snapshots = []
        snapshots = await provider_adapter.list_snapshots(container_name)
        snapshots_to_delete = await self.get_snapshots_to_cleanup(snapshots)

        for snapshot in snapshots_to_delete:
            snapshot_id = snapshot.get("id")
            snapshot_tag = snapshot.get("tag", "unknown")
            timestamp = snapshot.get("timestamp", "unknown")

            logger.info(f"Deleting expired snapshot: {snapshot_tag} (created: {timestamp})")

            result = await provider_adapter.delete_snapshot(snapshot_id)
            if result.get("status") == "deleted":
                deleted_snapshots.append(snapshot)

        return deleted_snapshots

    def get_policy_name(self) -> str:
        """Get the name of this cleanup policy."""
        return f"age_based_{self.retention_days}d"

    def _is_snapshot_expired(self, snapshot: Dict[str, Any], cutoff_date: datetime) -> bool:
        """Check if a snapshot is older than the cutoff date."""
        timestamp_str = snapshot.get("timestamp", "")
        if not timestamp_str:
            return False

        try:
            timestamp = datetime.fromisoformat(timestamp_str)
            return timestamp < cutoff_date
        except (ValueError, TypeError) as e:
            logger.warning(f"Could not parse timestamp for snapshot: {e}")
            return False