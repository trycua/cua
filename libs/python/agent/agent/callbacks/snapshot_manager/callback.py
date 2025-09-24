"""
Main snapshot manager callback class.
"""

from typing import Optional, Dict, Any, List
import logging

from ..base import AsyncCallbackHandler
from ..snapshots import (
    SnapshotCreator,
    MetadataManager,
    RetentionPolicyEnforcer,
    ProviderAdapter,
    SnapshotScheduler,
    StorageManager
)
from .operations import SnapshotOperations
from .cleanup import CleanupOperations

logger = logging.getLogger(__name__)


class SnapshotManagerCallback(AsyncCallbackHandler):
    """
    Manages container snapshots with configurable intervals and retention.
    """

    def __init__(self,
                 computer: Optional[Any] = None,
                 snapshot_interval: str = "manual",
                 max_snapshots: int = 10,
                 retention_days: int = 7,
                 metadata_dir: str = "/tmp/cua_snapshots",
                 auto_cleanup: bool = True,
                 snapshot_prefix: str = "cua-snapshot"):
        """Initialize the snapshot manager callback with specialized components."""
        # Initialize core components
        self.storage_manager = StorageManager(metadata_dir)
        self.provider_adapter = ProviderAdapter(computer)
        self.scheduler = SnapshotScheduler(snapshot_interval)
        self.snapshot_creator = SnapshotCreator(self.provider_adapter, snapshot_prefix)
        self.metadata_manager = MetadataManager(self.storage_manager)
        self.retention_enforcer = RetentionPolicyEnforcer(max_snapshots, retention_days, auto_cleanup)

        # Initialize operation handlers
        self.operations = SnapshotOperations(
            self.snapshot_creator,
            self.metadata_manager,
            self.retention_enforcer,
            self.provider_adapter
        )
        self.cleanup = CleanupOperations(
            self.retention_enforcer,
            self.metadata_manager,
            self.provider_adapter
        )

        # Store configuration
        self.computer = computer
        self.container_name = computer.config.name if computer and hasattr(computer, 'config') else None

        logger.info(f"SnapshotManagerCallback initialized with interval: {snapshot_interval}")

    def _get_container_name(self) -> Optional[str]:
        if not self.container_name:
            logger.warning("No container configured for snapshots; skipping operation")
            return None
        return self.container_name

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Create snapshot at run start if configured."""
        self.scheduler.start_new_run()

        if self.scheduler.should_create_snapshot_on_run_start():
            logger.info("Creating snapshot at run start")
            container_name = self._get_container_name()
            if not container_name:
                return
            await self.operations.create_and_save_snapshot(container_name, "run_start", self.scheduler)

    async def on_run_end(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]],
                         new_items: List[Dict[str, Any]]) -> None:
        """Create snapshot at run end if configured and perform cleanup."""
        container_name = self._get_container_name()
        if not container_name:
            return

        if self.scheduler.should_create_snapshot_on_run_end():
            logger.info("Creating snapshot at run end")
            await self.operations.create_and_save_snapshot(container_name, "run_end", self.scheduler)

        if self.retention_enforcer.auto_cleanup:
            logger.debug("Performing automatic cleanup of old snapshots")
            await self.cleanup.perform_cleanup(container_name)

    async def on_computer_call_end(self, item: Dict[str, Any], result: List[Dict[str, Any]]) -> None:
        """Create snapshot after each action if configured."""
        self.scheduler.increment_action_count()

        if self.scheduler.should_create_snapshot_on_action():
            trigger = self.scheduler.get_trigger_description("action", item)
            logger.info(f"Creating snapshot: {trigger}")
            container_name = self._get_container_name()
            if not container_name:
                return
            await self.operations.create_and_save_snapshot(container_name, trigger, self.scheduler)

    # Delegate public methods to operations handler
    async def create_manual_snapshot(self, description: str = "") -> Dict[str, Any]:
        """Create a manual snapshot."""
        return await self.operations.create_manual_snapshot(self.container_name, description)

    async def restore_snapshot(self, snapshot_id: str) -> Dict[str, Any]:
        """Restore to a specific snapshot."""
        return await self.operations.restore_snapshot(self.container_name, snapshot_id)

    async def list_snapshots(self) -> List[Dict[str, Any]]:
        """List all available snapshots."""
        return await self.operations.list_snapshots(self.container_name)

    async def delete_snapshot(self, snapshot_id: str) -> Dict[str, Any]:
        """Delete a specific snapshot."""
        container_name = self._get_container_name()
        if not container_name:
            return {"status": "error", "error": "No container configured"}
        return await self.operations.delete_snapshot(container_name, snapshot_id)

    def get_statistics(self) -> Dict[str, Any]:
        """Get statistics about the snapshot system."""
        return self.operations.get_statistics(
            self.container_name,
            self.scheduler,
            self.storage_manager,
            self.provider_adapter,
            self.retention_enforcer,
            self.metadata_manager
        )