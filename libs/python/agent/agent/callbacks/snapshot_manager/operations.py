"""
Snapshot operations for the manager.
"""

from typing import Optional, Dict, Any, List
import logging

logger = logging.getLogger(__name__)


class SnapshotOperations:
    """
    Handles snapshot operations for the manager.
    """

    def __init__(self, snapshot_creator, metadata_manager, retention_enforcer, provider_adapter):
        """Initialize with required components."""
        self.snapshot_creator = snapshot_creator
        self.metadata_manager = metadata_manager
        self.retention_enforcer = retention_enforcer
        self.provider_adapter = provider_adapter

    async def create_and_save_snapshot(self, container_name: Optional[str], trigger: str, scheduler) -> Optional[Dict[str, Any]]:
        """Create a snapshot and save its metadata."""
        if not container_name:
            logger.warning("No container name available")
            return None

        run_context = scheduler.get_run_context()

        snapshot = await self.snapshot_creator.create_snapshot(
            container_name,
            trigger,
            run_context
        )

        if snapshot and snapshot.get("status") != "error":
            retention_policy = {
                "max_snapshots": self.retention_enforcer.max_snapshots,
                "retention_days": self.retention_enforcer.retention_days
            }
            self.metadata_manager.save_metadata(
                container_name,
                snapshot,
                retention_policy
            )

            if self.retention_enforcer.auto_cleanup:
                await self.retention_enforcer.enforce_snapshot_limit(
                    self.provider_adapter,
                    container_name
                )

            return snapshot

        return None

    async def create_manual_snapshot(self, container_name: Optional[str], description: str = "") -> Dict[str, Any]:
        """Create a manual snapshot."""
        logger.info(f"Creating manual snapshot: {description}")

        if not container_name:
            return {"status": "error", "error": "No container configured"}

        trigger = f"manual: {description}" if description else "manual"

        snapshot = await self.snapshot_creator.create_snapshot(
            container_name,
            trigger,
            {}
        )

        if not snapshot or snapshot.get("status") == "error":
            return {"status": "error", "error": "Failed to create manual snapshot"}

        retention_policy = {
            "max_snapshots": self.retention_enforcer.max_snapshots,
            "retention_days": self.retention_enforcer.retention_days
        }
        self.metadata_manager.save_metadata(container_name, snapshot, retention_policy)

        return snapshot

    async def restore_snapshot(self, container_name: Optional[str], snapshot_id: str) -> Dict[str, Any]:
        """Restore to a specific snapshot."""
        if not container_name:
            return {"status": "error", "error": "No container configured"}

        logger.info(f"Restoring snapshot: {snapshot_id}")
        return await self.snapshot_creator.restore_snapshot(container_name, snapshot_id)

    async def list_snapshots(self, container_name: Optional[str]) -> List[Dict[str, Any]]:
        """List all available snapshots."""
        if not container_name:
            logger.warning("No container configured")
            return []

        return await self.provider_adapter.list_snapshots(container_name)

    async def delete_snapshot(self, container_name: Optional[str], snapshot_id: str) -> Dict[str, Any]:
        """Delete a specific snapshot."""
        logger.info(f"Deleting snapshot: {snapshot_id}")

        result = await self.provider_adapter.delete_snapshot(snapshot_id)

        if result.get("status") == "deleted" and container_name:
            self.metadata_manager.remove_snapshot_metadata(container_name, snapshot_id)

        return result

    def get_statistics(self, container_name, scheduler, storage_manager, provider_adapter,
                      retention_enforcer, metadata_manager) -> Dict[str, Any]:
        """Get statistics about the snapshot system."""
        stats = {
            "scheduler": scheduler.get_statistics(),
            "storage": storage_manager.get_storage_info(),
            "provider": provider_adapter.get_provider_info(),
            "retention": {
                "max_snapshots": retention_enforcer.max_snapshots,
                "retention_days": retention_enforcer.retention_days,
                "auto_cleanup": retention_enforcer.auto_cleanup
            }
        }

        if container_name:
            stats["snapshots"] = len(metadata_manager.get_snapshots_for_container(container_name))

        return stats