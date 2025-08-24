"""
Abstract interfaces for the snapshot management system.

These interfaces define the contracts that different components
must implement to be pluggable within the system.
"""

from abc import ABC, abstractmethod
from typing import Any, Dict, List, Optional

from .models import RestoreOptions, SnapshotMetadata


class SnapshotProvider(ABC):
    """
    Abstract interface for snapshot providers.

    A snapshot provider handles the actual creation and restoration
    of snapshots for a specific container technology (Docker, etc.).
    """

    @abstractmethod
    async def create_snapshot(
        self, container_id: str, metadata: SnapshotMetadata
    ) -> SnapshotMetadata:
        """
        Create a snapshot of the specified container.

        Args:
            container_id: ID of the container to snapshot
            metadata: Metadata for the snapshot (will be updated with storage info)

        Returns:
            Updated metadata with storage information filled in

        Raises:
            SnapshotError: If snapshot creation fails
        """
        pass

    @abstractmethod
    async def restore_snapshot(
        self, metadata: SnapshotMetadata, options: Optional[RestoreOptions] = None
    ) -> str:
        """
        Restore a container from a snapshot.

        Args:
            metadata: Metadata of the snapshot to restore
            options: Optional restore configuration

        Returns:
            ID of the restored container

        Raises:
            SnapshotError: If restore fails
        """
        pass

    @abstractmethod
    async def delete_snapshot(self, metadata: SnapshotMetadata) -> None:
        """
        Delete a snapshot and clean up its storage.

        Args:
            metadata: Metadata of the snapshot to delete

        Raises:
            SnapshotError: If deletion fails
        """
        pass

    @abstractmethod
    async def get_snapshot_size(self, metadata: SnapshotMetadata) -> int:
        """
        Get the size of a snapshot in bytes.

        Args:
            metadata: Metadata of the snapshot

        Returns:
            Size in bytes
        """
        pass

    @abstractmethod
    async def validate_container(self, container_id: str) -> bool:
        """
        Validate that a container exists and can be snapshotted.

        Args:
            container_id: ID of the container to validate

        Returns:
            True if container is valid for snapshotting
        """
        pass


class SnapshotStorage(ABC):
    """
    Abstract interface for snapshot metadata storage.

    Handles persistence of snapshot metadata and indexing
    for efficient retrieval and management.
    """

    @abstractmethod
    async def save_metadata(self, metadata: SnapshotMetadata) -> None:
        """
        Save snapshot metadata to persistent storage.

        Args:
            metadata: Metadata to save

        Raises:
            StorageError: If save operation fails
        """
        pass

    @abstractmethod
    async def load_metadata(self, snapshot_id: str) -> Optional[SnapshotMetadata]:
        """
        Load snapshot metadata by ID.

        Args:
            snapshot_id: ID of the snapshot

        Returns:
            Metadata if found, None otherwise
        """
        pass

    @abstractmethod
    async def list_snapshots(
        self, container_id: Optional[str] = None, limit: Optional[int] = None
    ) -> List[SnapshotMetadata]:
        """
        List snapshots, optionally filtered by container.

        Args:
            container_id: Optional filter by container ID
            limit: Optional limit on number of results

        Returns:
            List of snapshot metadata
        """
        pass

    @abstractmethod
    async def delete_metadata(self, snapshot_id: str) -> None:
        """
        Delete snapshot metadata from storage.

        Args:
            snapshot_id: ID of the snapshot to delete

        Raises:
            StorageError: If deletion fails
        """
        pass

    @abstractmethod
    async def update_metadata(self, metadata: SnapshotMetadata) -> None:
        """
        Update existing snapshot metadata.

        Args:
            metadata: Updated metadata

        Raises:
            StorageError: If update fails
        """
        pass

    @abstractmethod
    async def get_storage_stats(self) -> Dict[str, Any]:
        """
        Get storage statistics (total size, count, etc.).

        Returns:
            Dictionary with storage statistics
        """
        pass


class SnapshotError(Exception):
    """Base exception for snapshot operations."""

    pass


class StorageError(Exception):
    """Exception for storage operations."""

    pass


class ContainerNotFoundError(SnapshotError):
    """Exception raised when a container is not found."""

    pass


class SnapshotNotFoundError(SnapshotError):
    """Exception raised when a snapshot is not found."""

    pass
