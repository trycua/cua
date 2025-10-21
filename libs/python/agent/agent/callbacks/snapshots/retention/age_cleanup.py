"""
Age-based snapshot cleanup logic.
"""

from datetime import datetime, timedelta
from typing import List, Dict, Any, Tuple
import logging

logger = logging.getLogger(__name__)


class AgeBasedCleanup:
    """
    Handles age-based cleanup of snapshots.
    """

    def __init__(self, retention_days: int = 7):
        """
        Initialize age-based cleanup.

        Args:
            retention_days: Delete snapshots older than this many days
        """
        self.retention_days = retention_days

    async def cleanup_expired(self,
                             provider_adapter: Any,
                             container_name: str,
                             auto_cleanup: bool) -> List[Dict[str, Any]]:
        """
        Delete snapshots older than retention_days.

        Args:
            provider_adapter: Adapter for provider operations
            container_name: Name of the container
            auto_cleanup: Whether cleanup is enabled

        Returns:
            List of deleted snapshot information
        """
        if not auto_cleanup:
            return []

        deleted_snapshots = []
        cutoff_date = datetime.now() - timedelta(days=self.retention_days)

        logger.debug(f"Cleaning up snapshots older than {cutoff_date.isoformat()}")

        try:
            snapshots = await provider_adapter.list_snapshots(container_name)

            for snapshot in snapshots:
                if self._is_snapshot_expired(snapshot, cutoff_date):
                    snapshot_id = snapshot.get("id")
                    snapshot_tag = snapshot.get("tag", "unknown")
                    timestamp = snapshot.get("timestamp", "unknown")

                    logger.info(f"Deleting expired snapshot: {snapshot_tag} (created: {timestamp})")

                    result = await provider_adapter.delete_snapshot(snapshot_id)

                    if result.get("status") == "deleted":
                        deleted_snapshots.append(snapshot)
                    else:
                        logger.error(f"Failed to delete snapshot {snapshot_id}: {result.get('error')}")

        except Exception as e:
            logger.error(f"Error cleaning up old snapshots: {e}")

        return deleted_snapshots

    def find_expired_snapshots(self,
                              snapshots: List[Dict[str, Any]]) -> Tuple[List[Dict[str, Any]], int]:
        """
        Find snapshots that have expired based on age.

        Args:
            snapshots: List of snapshots to check

        Returns:
            Tuple of (expired snapshots, count)
        """
        cutoff_date = datetime.now() - timedelta(days=self.retention_days)
        expired = [s for s in snapshots if self._is_snapshot_expired(s, cutoff_date)]
        return expired, len(expired)

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