"""
Storage implementations for snapshot metadata.

Provides file-based storage for snapshot metadata with JSON serialization.
"""

import json
import logging
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

import aiofiles

from .interfaces import SnapshotStorage, StorageError
from .models import SnapshotMetadata

logger = logging.getLogger(__name__)


class FileSystemSnapshotStorage(SnapshotStorage):
    """
    File system-based storage for snapshot metadata.

    Stores each snapshot's metadata as a JSON file in a structured
    directory hierarchy for efficient access and management.
    """

    def __init__(self, base_path: str = ".snapshots"):
        """
        Initialize file system storage.

        Args:
            base_path: Base directory for storing snapshot metadata
        """
        self.base_path = Path(base_path)
        self.metadata_dir = self.base_path / "metadata"
        self.index_file = self.base_path / "index.json"

        # Create directory structure
        self._ensure_directories()

    def _ensure_directories(self) -> None:
        """Ensure the storage directory structure exists."""
        try:
            self.base_path.mkdir(parents=True, exist_ok=True)
            self.metadata_dir.mkdir(parents=True, exist_ok=True)
            logger.debug(f"Storage directories ensured at {self.base_path}")
        except OSError as e:
            raise StorageError(f"Failed to create storage directories: {e}")

    def _get_metadata_path(self, snapshot_id: str) -> Path:
        """
        Get the file path for a snapshot's metadata.

        Args:
            snapshot_id: ID of the snapshot

        Returns:
            Path to the metadata file
        """
        return self.metadata_dir / f"{snapshot_id}.json"

    async def _load_index(self) -> Dict[str, Any]:
        """
        Load the snapshot index from disk.

        Returns:
            Dictionary containing the snapshot index
        """
        if not self.index_file.exists():
            return {"snapshots": {}, "containers": {}, "last_updated": None}

        try:
            async with aiofiles.open(self.index_file, "r") as f:
                content = await f.read()
                return json.loads(content)
        except (json.JSONDecodeError, OSError) as e:
            logger.error(f"Error loading index: {e}")
            return {"snapshots": {}, "containers": {}, "last_updated": None}

    async def _save_index(self, index: Dict[str, Any]) -> None:
        """
        Save the snapshot index to disk.

        Args:
            index: Index dictionary to save
        """
        try:
            index["last_updated"] = datetime.now().isoformat()
            async with aiofiles.open(self.index_file, "w") as f:
                await f.write(json.dumps(index, indent=2))
        except OSError as e:
            logger.error(f"Error saving index: {e}")
            raise StorageError(f"Failed to save index: {e}")

    async def _update_index(self, metadata: SnapshotMetadata, remove: bool = False) -> None:
        """
        Update the snapshot index with new or removed metadata.

        Args:
            metadata: Snapshot metadata to add/remove
            remove: Whether to remove the entry (default: False = add)
        """
        index = await self._load_index()

        if remove:
            # Remove from index
            index["snapshots"].pop(metadata.snapshot_id, None)

            # Remove from container index
            container_snapshots = index["containers"].get(metadata.container_id, [])
            if metadata.snapshot_id in container_snapshots:
                container_snapshots.remove(metadata.snapshot_id)
                if not container_snapshots:
                    index["containers"].pop(metadata.container_id, None)
                else:
                    index["containers"][metadata.container_id] = container_snapshots
        else:
            # Add to index
            index["snapshots"][metadata.snapshot_id] = {
                "container_id": metadata.container_id,
                "container_name": metadata.container_name,
                "timestamp": metadata.timestamp.isoformat(),
                "trigger": metadata.trigger.value,
                "status": metadata.status.value,
                "size_bytes": metadata.size_bytes or 0,
            }

            # Add to container index
            if metadata.container_id not in index["containers"]:
                index["containers"][metadata.container_id] = []
            if metadata.snapshot_id not in index["containers"][metadata.container_id]:
                index["containers"][metadata.container_id].append(metadata.snapshot_id)

        await self._save_index(index)

    async def save_metadata(self, metadata: SnapshotMetadata) -> None:
        """
        Save snapshot metadata to persistent storage.

        Args:
            metadata: Metadata to save

        Raises:
            StorageError: If save operation fails
        """
        try:
            metadata_path = self._get_metadata_path(metadata.snapshot_id)

            # Convert to dictionary for JSON serialization
            metadata_dict = metadata.model_dump()

            # Ensure timestamp is serializable
            if isinstance(metadata_dict.get("timestamp"), datetime):
                metadata_dict["timestamp"] = metadata_dict["timestamp"].isoformat()

            async with aiofiles.open(metadata_path, "w") as f:
                await f.write(json.dumps(metadata_dict, indent=2))

            # Update index
            await self._update_index(metadata)

            logger.debug(f"Saved metadata for snapshot {metadata.snapshot_id}")

        except OSError as e:
            raise StorageError(f"Failed to save metadata for {metadata.snapshot_id}: {e}")
        except Exception as e:
            raise StorageError(f"Unexpected error saving metadata: {e}")

    async def load_metadata(self, snapshot_id: str) -> Optional[SnapshotMetadata]:
        """
        Load snapshot metadata by ID.

        Args:
            snapshot_id: ID of the snapshot

        Returns:
            Metadata if found, None otherwise
        """
        try:
            metadata_path = self._get_metadata_path(snapshot_id)

            if not metadata_path.exists():
                return None

            async with aiofiles.open(metadata_path, "r") as f:
                content = await f.read()
                metadata_dict = json.loads(content)

            # Convert timestamp back to datetime if it's a string
            if isinstance(metadata_dict.get("timestamp"), str):
                metadata_dict["timestamp"] = datetime.fromisoformat(metadata_dict["timestamp"])

            return SnapshotMetadata(**metadata_dict)

        except (json.JSONDecodeError, OSError) as e:
            logger.error(f"Error loading metadata for {snapshot_id}: {e}")
            return None
        except Exception as e:
            logger.error(f"Unexpected error loading metadata for {snapshot_id}: {e}")
            return None

    async def list_snapshots(
        self, container_id: Optional[str] = None, limit: Optional[int] = None
    ) -> List[SnapshotMetadata]:
        """
        List snapshots, optionally filtered by container.

        Args:
            container_id: Optional filter by container ID
            limit: Optional limit on number of results

        Returns:
            List of snapshot metadata, sorted by timestamp (newest first)
        """
        try:
            index = await self._load_index()

            if container_id:
                # Get snapshots for specific container
                snapshot_ids = index["containers"].get(container_id, [])
            else:
                # Get all snapshots
                snapshot_ids = list(index["snapshots"].keys())

            # Load metadata for each snapshot
            snapshots = []
            for snapshot_id in snapshot_ids:
                metadata = await self.load_metadata(snapshot_id)
                if metadata:
                    snapshots.append(metadata)

            # Sort by timestamp (newest first)
            snapshots.sort(key=lambda x: x.timestamp, reverse=True)

            # Apply limit if specified
            if limit:
                snapshots = snapshots[:limit]

            return snapshots

        except Exception as e:
            logger.error(f"Error listing snapshots: {e}")
            return []

    async def delete_metadata(self, snapshot_id: str) -> None:
        """
        Delete snapshot metadata from storage.

        Args:
            snapshot_id: ID of the snapshot to delete

        Raises:
            StorageError: If deletion fails
        """
        try:
            # Load metadata first to get container_id for index update
            metadata = await self.load_metadata(snapshot_id)

            # Delete metadata file
            metadata_path = self._get_metadata_path(snapshot_id)
            if metadata_path.exists():
                metadata_path.unlink()

            # Update index if we had metadata
            if metadata:
                await self._update_index(metadata, remove=True)

            logger.debug(f"Deleted metadata for snapshot {snapshot_id}")

        except OSError as e:
            raise StorageError(f"Failed to delete metadata for {snapshot_id}: {e}")
        except Exception as e:
            raise StorageError(f"Unexpected error deleting metadata: {e}")

    async def update_metadata(self, metadata: SnapshotMetadata) -> None:
        """
        Update existing snapshot metadata.

        Args:
            metadata: Updated metadata

        Raises:
            StorageError: If update fails
        """
        # For file-based storage, update is the same as save
        await self.save_metadata(metadata)

    async def get_storage_stats(self) -> Dict[str, Any]:
        """
        Get storage statistics (total size, count, etc.).

        Returns:
            Dictionary with storage statistics
        """
        try:
            index = await self._load_index()

            total_snapshots = len(index["snapshots"])
            total_size_bytes = sum(
                snapshot_info.get("size_bytes", 0) for snapshot_info in index["snapshots"].values()
            )

            container_count = len(index["containers"])

            # Calculate file system usage
            metadata_size = 0
            if self.metadata_dir.exists():
                for metadata_file in self.metadata_dir.glob("*.json"):
                    metadata_size += metadata_file.stat().st_size

            return {
                "total_snapshots": total_snapshots,
                "total_containers": container_count,
                "total_size_bytes": total_size_bytes,
                "total_size_gb": total_size_bytes / (1024**3),
                "metadata_size_bytes": metadata_size,
                "storage_path": str(self.base_path),
                "last_updated": index.get("last_updated"),
            }

        except Exception as e:
            logger.error(f"Error getting storage stats: {e}")
            return {
                "total_snapshots": 0,
                "total_containers": 0,
                "total_size_bytes": 0,
                "total_size_gb": 0,
                "metadata_size_bytes": 0,
                "storage_path": str(self.base_path),
                "last_updated": None,
                "error": str(e),
            }

    async def cleanup_orphaned_metadata(self) -> int:
        """
        Remove metadata files that don't exist in the index.

        Returns:
            Number of orphaned files cleaned up
        """
        try:
            index = await self._load_index()
            indexed_ids = set(index["snapshots"].keys())

            cleanup_count = 0
            if self.metadata_dir.exists():
                for metadata_file in self.metadata_dir.glob("*.json"):
                    snapshot_id = metadata_file.stem
                    if snapshot_id not in indexed_ids:
                        metadata_file.unlink()
                        cleanup_count += 1
                        logger.info(f"Cleaned up orphaned metadata file: {snapshot_id}")

            return cleanup_count

        except Exception as e:
            logger.error(f"Error during cleanup: {e}")
            return 0
