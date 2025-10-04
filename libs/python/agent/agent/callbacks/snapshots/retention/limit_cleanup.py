"""
Limit-based snapshot cleanup logic.
"""

from typing import List, Dict, Any, Tuple
import logging

logger = logging.getLogger(__name__)


class LimitBasedCleanup:
    """
    Handles count-based limit enforcement for snapshots.
    """

    def __init__(self, max_snapshots: int = 10):
        """
        Initialize limit-based cleanup.

        Args:
            max_snapshots: Maximum number of snapshots to retain
        """
        self.max_snapshots = max_snapshots

    async def enforce_limit(self,
                           provider_adapter: Any,
                           container_name: str,
                           auto_cleanup: bool) -> List[Dict[str, Any]]:
        """
        Delete oldest snapshots if over the maximum limit.

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

        try:
            snapshots = await provider_adapter.list_snapshots(container_name)

            if len(snapshots) <= self.max_snapshots:
                logger.debug(f"Within snapshot limit: {len(snapshots)} <= {self.max_snapshots}")
                return []

            logger.info(f"Enforcing snapshot limit: {len(snapshots)} > {self.max_snapshots}")

            sorted_snapshots = self._sort_snapshots_by_age(snapshots)
            snapshots_to_delete = sorted_snapshots[:-self.max_snapshots]

            for snapshot in snapshots_to_delete:
                snapshot_id = snapshot.get("id")
                snapshot_tag = snapshot.get("tag", "unknown")

                logger.info(f"Deleting snapshot over limit: {snapshot_tag} (ID: {snapshot_id[:8]})")

                result = await provider_adapter.delete_snapshot(snapshot_id)

                if result.get("status") == "deleted":
                    deleted_snapshots.append(snapshot)
                else:
                    logger.error(f"Failed to delete snapshot {snapshot_id}: {result.get('error')}")

        except Exception as e:
            logger.error(f"Error enforcing snapshot limit: {e}")

        return deleted_snapshots

    def find_over_limit_snapshots(self,
                                 snapshots: List[Dict[str, Any]]) -> Tuple[List[Dict[str, Any]], int]:
        """
        Find snapshots that exceed the count limit.

        Args:
            snapshots: List of snapshots to check

        Returns:
            Tuple of (snapshots to delete, count)
        """
        if len(snapshots) <= self.max_snapshots:
            return [], 0

        sorted_snapshots = self._sort_snapshots_by_age(snapshots)
        over_limit = sorted_snapshots[:-self.max_snapshots]
        return over_limit, len(over_limit)

    def _sort_snapshots_by_age(self, snapshots: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Sort snapshots by timestamp, oldest first."""
        return sorted(snapshots, key=lambda x: x.get("timestamp", ""))