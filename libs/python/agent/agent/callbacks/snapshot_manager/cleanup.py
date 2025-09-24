"""
Cleanup operations for snapshot manager.
"""

import logging

logger = logging.getLogger(__name__)


class CleanupOperations:
    """
    Handles cleanup operations for the snapshot manager.
    """

    def __init__(self, retention_enforcer, metadata_manager, provider_adapter):
        """Initialize with required components."""
        self.retention_enforcer = retention_enforcer
        self.metadata_manager = metadata_manager
        self.provider_adapter = provider_adapter

    async def perform_cleanup(self, container_name: str) -> None:
        """Perform retention policy cleanup."""
        if not container_name:
            return

        # Clean up old snapshots
        deleted = await self.retention_enforcer.cleanup_old_snapshots(
            self.provider_adapter,
            container_name
        )

        # Remove metadata for deleted snapshots
        for snapshot in deleted:
            snapshot_id = snapshot.get("id")
            if snapshot_id:
                self.metadata_manager.remove_snapshot_metadata(
                    container_name,
                    snapshot_id
                )

        # Also enforce count limit
        deleted = await self.retention_enforcer.enforce_snapshot_limit(
            self.provider_adapter,
            container_name
        )

        # Remove metadata for deleted snapshots
        for snapshot in deleted:
            snapshot_id = snapshot.get("id")
            if snapshot_id:
                self.metadata_manager.remove_snapshot_metadata(
                    container_name,
                    snapshot_id
                )