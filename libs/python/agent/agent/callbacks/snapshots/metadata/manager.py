"""
Core metadata management for snapshots.
"""

from datetime import datetime
from typing import Dict, Any, List, Optional
import logging

from .query import MetadataQuery

logger = logging.getLogger(__name__)


class MetadataManager:
    """
    Manages snapshot metadata persistence and retrieval.
    """

    def __init__(self, storage_manager: Any):
        """
        Initialize the metadata manager.

        Args:
            storage_manager: Storage manager for file I/O operations
        """
        self.storage_manager = storage_manager
        self.query = MetadataQuery(storage_manager)

    def save_metadata(self,
                     container_name: str,
                     snapshot: Dict[str, Any],
                     retention_policy: Optional[Dict[str, Any]] = None) -> None:
        """
        Save snapshot metadata to persistent storage.

        Args:
            container_name: Name of the container
            snapshot: Snapshot information to save
            retention_policy: Current retention policy settings
        """
        metadata_file = self.storage_manager.get_metadata_path(container_name)

        data = self._load_or_initialize_metadata(metadata_file)

        data["snapshots"].append(snapshot)

        if retention_policy:
            data["retention_policy"] = {
                **retention_policy,
                "last_updated": datetime.now().isoformat()
            }

        try:
            self.storage_manager.write_json_file(metadata_file, data)
            logger.debug(f"Saved snapshot metadata for {container_name}")
        except Exception as e:
            logger.error(f"Failed to save metadata: {e}")

    def load_metadata(self, container_name: str) -> Dict[str, Any]:
        """Load metadata for a specific container."""
        metadata_file = self.storage_manager.get_metadata_path(container_name)
        return self._load_or_initialize_metadata(metadata_file)

    def update_retention_policy(self,
                               container_name: str,
                               max_snapshots: int,
                               retention_days: int) -> None:
        """Update the retention policy for a container."""
        metadata_file = self.storage_manager.get_metadata_path(container_name)
        data = self._load_or_initialize_metadata(metadata_file)

        data["retention_policy"] = {
            "max_snapshots": max_snapshots,
            "retention_days": retention_days,
            "last_cleanup": datetime.now().isoformat()
        }

        try:
            self.storage_manager.write_json_file(metadata_file, data)
            logger.debug(f"Updated retention policy for {container_name}")
        except Exception as e:
            logger.error(f"Failed to update retention policy: {e}")

    def remove_snapshot_metadata(self,
                                container_name: str,
                                snapshot_id: str) -> None:
        """Remove metadata for a deleted snapshot."""
        metadata_file = self.storage_manager.get_metadata_path(container_name)
        data = self._load_or_initialize_metadata(metadata_file)

        original_count = len(data["snapshots"])
        data["snapshots"] = [
            s for s in data["snapshots"]
            if s.get("id") != snapshot_id
        ]

        if len(data["snapshots"]) < original_count:
            try:
                self.storage_manager.write_json_file(metadata_file, data)
                logger.debug(f"Removed metadata for snapshot {snapshot_id}")
            except Exception as e:
                logger.error(f"Failed to remove snapshot metadata: {e}")

    def get_snapshots_for_container(self, container_name: str) -> List[Dict[str, Any]]:
        """Get all snapshots for a specific container."""
        return self.query.get_snapshots_for_container(container_name)

    def get_snapshot_by_id(self,
                          container_name: str,
                          snapshot_id: str) -> Optional[Dict[str, Any]]:
        """Get metadata for a specific snapshot."""
        return self.query.get_snapshot_by_id(container_name, snapshot_id)

    def _load_or_initialize_metadata(self, metadata_file: str) -> Dict[str, Any]:
        """Load metadata from file or create initial structure."""
        existing_data = self.storage_manager.read_json_file(metadata_file)

        if existing_data:
            return existing_data

        return {
            "snapshots": [],
            "retention_policy": {},
            "created_at": datetime.now().isoformat()
        }