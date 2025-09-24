"""
Snapshot manager callback for managing container snapshots during agent runs.

This callback provides configurable snapshot intervals, retention policies,
and automatic cleanup to prevent unbounded storage growth. This is the main
orchestrator that delegates to specialized components for each responsibility.
"""

from typing import Optional, Dict, Any, List
import logging

from .base import AsyncCallbackHandler
from .snapshots import (
    SnapshotCreator,
    MetadataManager,
    RetentionPolicyEnforcer,
    ProviderAdapter,
    SnapshotScheduler,
    StorageManager
)

logger = logging.getLogger(__name__)


class SnapshotManagerCallback(AsyncCallbackHandler):
    """
    Manages container snapshots with configurable intervals and retention.
    Works as a pluggable component in the Agent SDK callback system.

    This class orchestrates the various snapshot components, delegating
    specific responsibilities to specialized classes following the
    Single Responsibility Principle.
    """

    def __init__(self,
                 computer: Optional[Any] = None,  # Computer instance
                 snapshot_interval: str = "manual",  # "manual", "every_action", "run_start", "run_end", "run_boundaries"
                 max_snapshots: int = 10,
                 retention_days: int = 7,
                 metadata_dir: str = "/tmp/cua_snapshots",
                 auto_cleanup: bool = True,
                 snapshot_prefix: str = "cua-snapshot"):
        """
        Initialize the snapshot manager callback with specialized components.

        Args:
            computer: Computer instance with Docker provider
            snapshot_interval: When to create snapshots:
                - "manual": Only create snapshots when explicitly called
                - "every_action": Create snapshot after each computer action
                - "run_start": Create snapshot at the start of each run
                - "run_end": Create snapshot at the end of each run
                - "run_boundaries": Create snapshots at both start and end of runs
            max_snapshots: Maximum number of snapshots to retain
            retention_days: Delete snapshots older than this many days
            metadata_dir: Directory to store snapshot metadata
            auto_cleanup: Whether to automatically cleanup old snapshots
            snapshot_prefix: Prefix for snapshot names
        """
        # Initialize components following SRP
        self.storage_manager = StorageManager(metadata_dir)
        self.provider_adapter = ProviderAdapter(computer)
        self.scheduler = SnapshotScheduler(snapshot_interval)
        self.snapshot_creator = SnapshotCreator(self.provider_adapter, snapshot_prefix)
        self.metadata_manager = MetadataManager(self.storage_manager)
        self.retention_enforcer = RetentionPolicyEnforcer(max_snapshots, retention_days, auto_cleanup)

        # Store configuration
        self.computer = computer
        self.container_name = computer.config.name if computer and hasattr(computer, 'config') else None

        logger.info(f"SnapshotManagerCallback initialized with interval: {snapshot_interval}, "
                   f"max_snapshots: {max_snapshots}, retention_days: {retention_days}")

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Create snapshot at run start if configured."""
        self.scheduler.start_new_run()

        if self.scheduler.should_create_snapshot_on_run_start():
            logger.info("Creating snapshot at run start")
            await self._create_and_save_snapshot("run_start")

    async def on_run_end(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]],
                         new_items: List[Dict[str, Any]]) -> None:
        """Create snapshot at run end if configured and perform cleanup."""
        if self.scheduler.should_create_snapshot_on_run_end():
            logger.info("Creating snapshot at run end")
            await self._create_and_save_snapshot("run_end")

        # Perform retention enforcement
        if self.retention_enforcer.auto_cleanup and self.container_name:
            logger.debug("Performing automatic cleanup of old snapshots")
            await self._perform_cleanup()

    async def on_computer_call_end(self, item: Dict[str, Any], result: List[Dict[str, Any]]) -> None:
        """Create snapshot after each action if configured."""
        self.scheduler.increment_action_count()

        if self.scheduler.should_create_snapshot_on_action():
            trigger = self.scheduler.get_trigger_description("action", item)
            logger.info(f"Creating snapshot: {trigger}")
            await self._create_and_save_snapshot(trigger)

    async def _create_and_save_snapshot(self, trigger: str) -> Optional[Dict[str, Any]]:
        """
        Create a snapshot and save its metadata.

        Args:
            trigger: Description of what triggered the snapshot

        Returns:
            Snapshot information or None if failed
        """
        if not self.container_name:
            logger.warning("No container name available")
            return None

        # Add run context to metadata
        run_context = self.scheduler.get_run_context()

        # Create the snapshot
        snapshot = await self.snapshot_creator.create_snapshot(
            self.container_name,
            trigger,
            run_context
        )

        if snapshot and snapshot.get("status") != "error":
            # Save metadata
            retention_policy = {
                "max_snapshots": self.retention_enforcer.max_snapshots,
                "retention_days": self.retention_enforcer.retention_days
            }
            self.metadata_manager.save_metadata(
                self.container_name,
                snapshot,
                retention_policy
            )

            # Enforce retention limits
            if self.retention_enforcer.auto_cleanup:
                await self.retention_enforcer.enforce_snapshot_limit(
                    self.provider_adapter,
                    self.container_name
                )

            return snapshot

        return None

    async def _perform_cleanup(self) -> None:
        """Perform retention policy cleanup."""
        if not self.container_name:
            return

        # Clean up old snapshots
        deleted = await self.retention_enforcer.cleanup_old_snapshots(
            self.provider_adapter,
            self.container_name
        )

        # Remove metadata for deleted snapshots
        for snapshot in deleted:
            snapshot_id = snapshot.get("id")
            if snapshot_id:
                self.metadata_manager.remove_snapshot_metadata(
                    self.container_name,
                    snapshot_id
                )

        # Also enforce count limit
        deleted = await self.retention_enforcer.enforce_snapshot_limit(
            self.provider_adapter,
            self.container_name
        )

        # Remove metadata for deleted snapshots
        for snapshot in deleted:
            snapshot_id = snapshot.get("id")
            if snapshot_id:
                self.metadata_manager.remove_snapshot_metadata(
                    self.container_name,
                    snapshot_id
                )

    async def create_manual_snapshot(self, description: str = "") -> Dict[str, Any]:
        """
        Public method for manual snapshot creation.

        Args:
            description: Optional description for the snapshot

        Returns:
            Dictionary with snapshot information
        """
        logger.info(f"Creating manual snapshot: {description}")

        if not self.container_name:
            return {"status": "error", "error": "No container configured"}

        trigger = f"manual: {description}" if description else "manual"
        result = await self._create_and_save_snapshot(trigger)

        if not result:
            return {"status": "error", "error": "Failed to create manual snapshot"}
        return result

    async def restore_snapshot(self, snapshot_id: str) -> Dict[str, Any]:
        """
        Restore to a specific snapshot.

        Args:
            snapshot_id: ID of the snapshot to restore

        Returns:
            Dictionary with restore status
        """
        if not self.container_name:
            return {"status": "error", "error": "No container configured"}

        logger.info(f"Restoring snapshot: {snapshot_id}")
        return await self.snapshot_creator.restore_snapshot(
            self.container_name,
            snapshot_id
        )

    async def list_snapshots(self) -> List[Dict[str, Any]]:
        """
        List all available snapshots.

        Returns:
            List of snapshot dictionaries
        """
        if not self.container_name:
            logger.warning("No container configured")
            return []

        return await self.provider_adapter.list_snapshots(self.container_name)

    async def delete_snapshot(self, snapshot_id: str) -> Dict[str, Any]:
        """
        Delete a specific snapshot.

        Args:
            snapshot_id: ID of the snapshot to delete

        Returns:
            Dictionary with deletion status
        """
        logger.info(f"Deleting snapshot: {snapshot_id}")

        result = await self.provider_adapter.delete_snapshot(snapshot_id)

        # Remove metadata if deletion succeeded
        if result.get("status") == "deleted" and self.container_name:
            self.metadata_manager.remove_snapshot_metadata(
                self.container_name,
                snapshot_id
            )

        return result

    def get_statistics(self) -> Dict[str, Any]:
        """
        Get statistics about the snapshot system.

        Returns:
            Dictionary with various statistics
        """
        stats = {
            "scheduler": self.scheduler.get_statistics(),
            "storage": self.storage_manager.get_storage_info(),
            "provider": self.provider_adapter.get_provider_info(),
            "retention": {
                "max_snapshots": self.retention_enforcer.max_snapshots,
                "retention_days": self.retention_enforcer.retention_days,
                "auto_cleanup": self.retention_enforcer.auto_cleanup
            }
        }

        if self.container_name:
            stats["snapshots"] = len(self.metadata_manager.get_snapshots_for_container(self.container_name))

        return stats