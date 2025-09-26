"""
Query operations for snapshot metadata.
"""

from typing import Dict, Any, List, Optional
import logging

logger = logging.getLogger(__name__)


class MetadataQuery:
    """
    Handles querying and filtering of snapshot metadata.
    """

    def __init__(self, storage_manager: Any):
        """
        Initialize metadata query operations.

        Args:
            storage_manager: Storage manager for file I/O operations
        """
        self.storage_manager = storage_manager

    def get_snapshots_for_container(self, container_name: str) -> List[Dict[str, Any]]:
        """
        Get all snapshots for a specific container.

        Args:
            container_name: Name of the container

        Returns:
            List of snapshot dictionaries
        """
        data = self._load_metadata(container_name)
        return data.get("snapshots", [])

    def get_snapshot_by_id(self,
                          container_name: str,
                          snapshot_id: str) -> Optional[Dict[str, Any]]:
        """
        Get metadata for a specific snapshot.

        Args:
            container_name: Name of the container
            snapshot_id: ID of the snapshot

        Returns:
            Snapshot dictionary or None if not found
        """
        snapshots = self.get_snapshots_for_container(container_name)
        for snapshot in snapshots:
            if snapshot.get("id") == snapshot_id:
                return snapshot
        return None

    def find_snapshots_by_trigger(self,
                                 container_name: str,
                                 trigger: str) -> List[Dict[str, Any]]:
        """
        Find snapshots by trigger type.

        Args:
            container_name: Name of the container
            trigger: Trigger type to search for

        Returns:
            List of matching snapshots
        """
        snapshots = self.get_snapshots_for_container(container_name)
        return [s for s in snapshots if s.get("trigger") == trigger]

    def get_recent_snapshots(self,
                            container_name: str,
                            limit: int = 10) -> List[Dict[str, Any]]:
        """
        Get the most recent snapshots.

        Args:
            container_name: Name of the container
            limit: Maximum number of snapshots to return

        Returns:
            List of recent snapshots
        """
        snapshots = self.get_snapshots_for_container(container_name)
        sorted_snapshots = sorted(
            snapshots,
            key=lambda x: x.get("timestamp", ""),
            reverse=True
        )
        return sorted_snapshots[:limit]

    def get_retention_policy(self, container_name: str) -> Dict[str, Any]:
        """
        Get the retention policy for a container.

        Args:
            container_name: Name of the container

        Returns:
            Retention policy dictionary
        """
        data = self._load_metadata(container_name)
        return data.get("retention_policy", {})

    def _load_metadata(self, container_name: str) -> Dict[str, Any]:
        """Load metadata for a container."""
        metadata_file = self.storage_manager.get_metadata_path(container_name)
        existing_data = self.storage_manager.read_json_file(metadata_file)

        if existing_data:
            return existing_data

        return {
            "snapshots": [],
            "retention_policy": {},
            "created_at": ""
        }