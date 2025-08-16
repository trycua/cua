"""
Main snapshot manager that orchestrates snapshot operations.

This is the primary interface for the snapshot system, coordinating
between providers, storage, and cleanup policies.
"""

import asyncio
import logging
import uuid
from datetime import datetime, timedelta
from typing import Any, Dict, List, Optional

from .interfaces import SnapshotError, SnapshotProvider, SnapshotStorage
from .models import (
    RestoreOptions,
    SnapshotConfig,
    SnapshotMetadata,
    SnapshotStatus,
    SnapshotTrigger,
)
from .providers import DockerSnapshotProvider
from .storage import FileSystemSnapshotStorage

logger = logging.getLogger(__name__)


class SnapshotManager:
    """
    Main coordinator for snapshot operations.

    Manages the creation, storage, restoration, and cleanup of container
    snapshots according to configured policies and triggers.
    """

    def __init__(
        self,
        provider: Optional[SnapshotProvider] = None,
        storage: Optional[SnapshotStorage] = None,
        config: Optional[SnapshotConfig] = None,
    ):
        """
        Initialize the snapshot manager.

        Args:
            provider: Snapshot provider (defaults to DockerSnapshotProvider)
            storage: Storage backend (defaults to FileSystemSnapshotStorage)
            config: Configuration (defaults to SnapshotConfig with sensible defaults)
        """
        self.config = config or SnapshotConfig()
        self.provider = provider or DockerSnapshotProvider()
        self.storage = storage or FileSystemSnapshotStorage(base_path=self.config.storage_path)

        # Track active operations to prevent conflicts
        self._active_operations: Dict[str, str] = {}  # container_id -> operation_type
        self._operation_lock = asyncio.Lock()

        logger.info(f"SnapshotManager initialized with {type(self.provider).__name__} provider")

    def _generate_snapshot_id(self) -> str:
        """Generate a unique snapshot ID."""
        return str(uuid.uuid4())

    def _format_snapshot_name(self, metadata: SnapshotMetadata) -> str:
        """
        Format a human-readable snapshot name based on the configured pattern.

        Args:
            metadata: Snapshot metadata

        Returns:
            Formatted snapshot name
        """
        timestamp = metadata.timestamp.strftime("%Y%m%d_%H%M%S")

        format_vars = {
            "container_name": metadata.container_name,
            "trigger": metadata.trigger.value,
            "timestamp": timestamp,
            "snapshot_id": metadata.snapshot_id[:8],  # Short version
        }

        try:
            return self.config.naming_pattern.format(**format_vars)
        except KeyError as e:
            logger.warning(f"Invalid naming pattern variable {e}, using default")
            return f"{metadata.container_name}_{metadata.trigger.value}_{timestamp}"

    async def _check_operation_conflict(self, container_id: str, operation: str) -> None:
        """
        Check if there's a conflicting operation in progress for this container.

        Args:
            container_id: ID of the container
            operation: Type of operation being attempted

        Raises:
            SnapshotError: If there's a conflicting operation
        """
        async with self._operation_lock:
            if container_id in self._active_operations:
                active_op = self._active_operations[container_id]
                raise SnapshotError(f"Container {container_id} is already {active_op}")

            self._active_operations[container_id] = operation

    async def _clear_operation(self, container_id: str) -> None:
        """Clear the active operation for a container."""
        async with self._operation_lock:
            self._active_operations.pop(container_id, None)

    async def create_snapshot(
        self,
        container_id: str,
        trigger: SnapshotTrigger = SnapshotTrigger.MANUAL,
        description: Optional[str] = None,
        action_context: Optional[str] = None,
        run_id: Optional[str] = None,
        labels: Optional[Dict[str, str]] = None,
    ) -> SnapshotMetadata:
        """
        Create a new snapshot of the specified container.

        Args:
            container_id: ID or name of the container to snapshot
            trigger: What triggered this snapshot
            description: Optional human-readable description
            action_context: Context of the action that triggered the snapshot
            run_id: ID of the agent run this snapshot belongs to
            labels: Additional labels/tags for the snapshot

        Returns:
            Metadata of the created snapshot

        Raises:
            SnapshotError: If snapshot creation fails
        """
        await self._check_operation_conflict(container_id, "creating snapshot")

        try:
            logger.info(f"Creating snapshot for container {container_id}, trigger: {trigger.value}")

            # Validate container exists
            if not await self.provider.validate_container(container_id):
                raise SnapshotError(f"Container {container_id} is not valid for snapshotting")

            # Create metadata
            snapshot_id = self._generate_snapshot_id()
            metadata = SnapshotMetadata(
                snapshot_id=snapshot_id,
                container_id=container_id,
                container_name=container_id,  # Will be updated by provider
                trigger=trigger,
                description=description,
                action_context=action_context,
                run_id=run_id,
                labels=labels or {},
            )

            # Check if we need to enforce limits before creating
            await self._enforce_snapshot_limits(container_id)

            # Create the snapshot using the provider
            metadata = await self.provider.create_snapshot(container_id, metadata)

            # Save metadata to storage
            await self.storage.save_metadata(metadata)

            logger.info(f"Successfully created snapshot {snapshot_id} for container {container_id}")
            return metadata

        except Exception as e:
            logger.error(f"Failed to create snapshot for container {container_id}: {e}")
            raise
        finally:
            await self._clear_operation(container_id)

    async def restore_snapshot(
        self, snapshot_id: str, options: Optional[RestoreOptions] = None
    ) -> str:
        """
        Restore a container from a snapshot.

        Args:
            snapshot_id: ID of the snapshot to restore
            options: Optional restore configuration

        Returns:
            ID of the restored container

        Raises:
            SnapshotError: If restore fails
        """
        logger.info(f"Restoring snapshot {snapshot_id}")

        # Load metadata
        metadata = await self.storage.load_metadata(snapshot_id)
        if not metadata:
            raise SnapshotError(f"Snapshot {snapshot_id} not found")

        if metadata.status != SnapshotStatus.COMPLETED:
            raise SnapshotError(
                f"Snapshot {snapshot_id} is not in completed state (status: {metadata.status})"
            )

        try:
            # Check for operation conflicts on the original container
            # (in case someone tries to restore while creating a snapshot)
            await self._check_operation_conflict(metadata.container_id, "restoring snapshot")

            # Restore using the provider
            container_id = await self.provider.restore_snapshot(metadata, options)

            # Update metadata
            metadata.restoration_count += 1
            await self.storage.update_metadata(metadata)

            logger.info(
                f"Successfully restored container {container_id} from snapshot {snapshot_id}"
            )
            return container_id

        except Exception as e:
            logger.error(f"Failed to restore snapshot {snapshot_id}: {e}")
            raise
        finally:
            await self._clear_operation(metadata.container_id)

    async def delete_snapshot(self, snapshot_id: str) -> None:
        """
        Delete a snapshot and clean up its storage.

        Args:
            snapshot_id: ID of the snapshot to delete

        Raises:
            SnapshotError: If deletion fails
        """
        logger.info(f"Deleting snapshot {snapshot_id}")

        # Load metadata
        metadata = await self.storage.load_metadata(snapshot_id)
        if not metadata:
            logger.warning(
                f"Snapshot {snapshot_id} metadata not found, but proceeding with cleanup"
            )
            return

        try:
            # Delete using the provider
            await self.provider.delete_snapshot(metadata)

            # Remove metadata from storage
            await self.storage.delete_metadata(snapshot_id)

            logger.info(f"Successfully deleted snapshot {snapshot_id}")

        except Exception as e:
            logger.error(f"Failed to delete snapshot {snapshot_id}: {e}")
            raise

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
        return await self.storage.list_snapshots(container_id, limit)

    async def get_snapshot(self, snapshot_id: str) -> Optional[SnapshotMetadata]:
        """
        Get metadata for a specific snapshot.

        Args:
            snapshot_id: ID of the snapshot

        Returns:
            Snapshot metadata if found, None otherwise
        """
        return await self.storage.load_metadata(snapshot_id)

    async def _enforce_snapshot_limits(self, container_id: str) -> None:
        """
        Enforce snapshot limits by cleaning up old snapshots if necessary.

        Args:
            container_id: ID of the container
        """
        # Check per-container limit
        container_snapshots = await self.storage.list_snapshots(container_id)
        if len(container_snapshots) >= self.config.max_snapshots_per_container:
            # Delete oldest snapshots to make room
            snapshots_to_delete = container_snapshots[self.config.max_snapshots_per_container - 1 :]
            for snapshot in snapshots_to_delete:
                try:
                    await self.delete_snapshot(snapshot.snapshot_id)
                    logger.info(
                        f"Auto-deleted old snapshot {snapshot.snapshot_id} due to per-container limit"
                    )
                except Exception as e:
                    logger.error(f"Failed to auto-delete snapshot {snapshot.snapshot_id}: {e}")

        # Check total snapshot limit
        all_snapshots = await self.storage.list_snapshots()
        if len(all_snapshots) >= self.config.max_total_snapshots:
            # Delete oldest snapshots globally
            snapshots_to_delete = all_snapshots[self.config.max_total_snapshots - 1 :]
            for snapshot in snapshots_to_delete:
                try:
                    await self.delete_snapshot(snapshot.snapshot_id)
                    logger.info(
                        f"Auto-deleted old snapshot {snapshot.snapshot_id} due to total limit"
                    )
                except Exception as e:
                    logger.error(f"Failed to auto-delete snapshot {snapshot.snapshot_id}: {e}")

        # Check storage size limit
        stats = await self.storage.get_storage_stats()
        if stats["total_size_gb"] > self.config.max_storage_size_gb:
            # Delete oldest snapshots until under limit
            all_snapshots = await self.storage.list_snapshots()
            for snapshot in reversed(all_snapshots):  # Oldest first
                try:
                    await self.delete_snapshot(snapshot.snapshot_id)
                    logger.info(
                        f"Auto-deleted snapshot {snapshot.snapshot_id} due to storage size limit"
                    )

                    # Check if we're now under the limit
                    stats = await self.storage.get_storage_stats()
                    if stats["total_size_gb"] <= self.config.max_storage_size_gb:
                        break
                except Exception as e:
                    logger.error(f"Failed to auto-delete snapshot {snapshot.snapshot_id}: {e}")

    async def cleanup_old_snapshots(self, max_age_days: Optional[int] = None) -> int:
        """
        Clean up snapshots older than the specified age.

        Args:
            max_age_days: Maximum age in days (defaults to config.auto_cleanup_days)

        Returns:
            Number of snapshots cleaned up
        """
        max_age = max_age_days or self.config.auto_cleanup_days
        cutoff_date = datetime.now() - timedelta(days=max_age)

        logger.info(f"Cleaning up snapshots older than {max_age} days (before {cutoff_date})")

        all_snapshots = await self.storage.list_snapshots()
        cleanup_count = 0

        for snapshot in all_snapshots:
            if snapshot.timestamp < cutoff_date:
                try:
                    await self.delete_snapshot(snapshot.snapshot_id)
                    cleanup_count += 1
                    logger.info(f"Cleaned up old snapshot {snapshot.snapshot_id}")
                except Exception as e:
                    logger.error(f"Failed to clean up snapshot {snapshot.snapshot_id}: {e}")

        logger.info(f"Cleaned up {cleanup_count} old snapshots")
        return cleanup_count

    async def get_storage_stats(self) -> Dict[str, Any]:
        """
        Get storage statistics and system information.

        Returns:
            Dictionary with storage statistics
        """
        return await self.storage.get_storage_stats()

    async def should_create_snapshot(self, trigger: SnapshotTrigger) -> bool:
        """
        Check if a snapshot should be created for the given trigger.

        Args:
            trigger: The trigger type

        Returns:
            True if snapshot should be created
        """
        return trigger in self.config.triggers
