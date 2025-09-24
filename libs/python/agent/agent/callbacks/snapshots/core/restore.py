"""
Snapshot restoration operations.
"""

from typing import Dict, Any
import logging

logger = logging.getLogger(__name__)


class RestoreOperations:
    """
    Handles snapshot restoration operations.
    """

    def __init__(self, provider_adapter: Any):
        """
        Initialize restore operations.

        Args:
            provider_adapter: Adapter for interacting with the VM provider
        """
        self.provider_adapter = provider_adapter

    async def restore(self, container_name: str, snapshot_id: str) -> Dict[str, Any]:
        """
        Restore a container to a specific snapshot.

        Args:
            container_name: Name of the container to restore
            snapshot_id: ID of the snapshot to restore

        Returns:
            Dictionary with restore status
        """
        if not await self.provider_adapter.validate_provider():
            return {
                "status": "error",
                "error": "Provider does not support snapshots"
            }

        try:
            logger.info(f"Restoring snapshot {snapshot_id} for container {container_name}")

            result = await self.provider_adapter.restore_snapshot(
                container_name,
                snapshot_id
            )

            if result.get("status") == "restored":
                logger.info(f"Successfully restored snapshot {snapshot_id}")
                await self._handle_post_restore(container_name)
            else:
                logger.error(f"Failed to restore snapshot: {result.get('error', 'Unknown error')}")

            return result

        except Exception as e:
            logger.error(f"Error restoring snapshot: {e}")
            return {
                "status": "error",
                "error": str(e)
            }

    async def _handle_post_restore(self, container_name: str) -> None:
        """Handle post-restoration tasks."""
        try:
            if hasattr(self.provider_adapter, 'reconnect_after_restore'):
                await self.provider_adapter.reconnect_after_restore(container_name)
        except Exception as e:
            logger.warning(f"Post-restore handling failed: {e}")