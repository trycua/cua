"""
Snapshot CRUD operations through providers.
"""

from typing import Optional, Dict, Any
import logging

logger = logging.getLogger(__name__)


class SnapshotOperations:
    """
    Handles snapshot CRUD operations through the provider.
    """

    def __init__(self, adapter):
        """
        Initialize snapshot operations.

        Args:
            adapter: Parent ProviderAdapter instance
        """
        self.adapter = adapter

    async def create(self, container_name: str, snapshot_name: str,
                    metadata: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        """Create a snapshot through the provider."""
        if not await self.adapter.validate_provider():
            return {"status": "error", "error": "Provider not available or doesn't support snapshots"}

        try:
            if self.adapter._provider is None:
                return {"status": "error", "error": "Provider not available"}

            result = await self.adapter._provider.create_snapshot(
                container_name,
                snapshot_name=snapshot_name,
                metadata=metadata
            )

            if result and "id" in result and "tag" not in result:
                result["tag"] = snapshot_name

            return result

        except NotImplementedError:
            logger.error("Provider does not implement create_snapshot")
            return {"status": "error", "error": "Provider does not support snapshots"}
        except Exception as e:
            logger.error(f"Provider error creating snapshot: {e}")
            return {"status": "error", "error": str(e)}

    async def restore(self, container_name: str, snapshot_id: str) -> Dict[str, Any]:
        """Restore a snapshot through the provider."""
        if not await self.adapter.validate_provider():
            return {"status": "error", "error": "Provider not available or doesn't support snapshots"}

        try:
            if self.adapter._provider is None:
                return {"status": "error", "error": "Provider not available"}

            result = await self.adapter._provider.restore_snapshot(container_name, snapshot_id)

            if not result:
                return {"status": "error", "error": "Restore operation returned no result"}

            await self._reconnect_after_restore(container_name)

            return result

        except NotImplementedError:
            logger.error("Provider does not implement restore_snapshot")
            return {"status": "error", "error": "Provider does not support snapshot restoration"}
        except Exception as e:
            logger.error(f"Provider error restoring snapshot: {e}")
            return {"status": "error", "error": str(e)}

    async def delete(self, snapshot_id: str) -> Dict[str, Any]:
        """Delete a snapshot through the provider."""
        if not await self.adapter.validate_provider():
            return {"status": "error", "error": "Provider not available or doesn't support snapshots"}

        try:
            if self.adapter._provider is None:
                return {"status": "error", "error": "Provider not available"}

            result = await self.adapter._provider.delete_snapshot(snapshot_id)

            if not result:
                return {"status": "deleted"}

            return result

        except NotImplementedError:
            logger.error("Provider does not implement delete_snapshot")
            return {"status": "error", "error": "Provider does not support snapshot deletion"}
        except Exception as e:
            logger.error(f"Provider error deleting snapshot: {e}")
            return {"status": "error", "error": str(e)}

    async def _reconnect_after_restore(self, container_name: str) -> None:
        """Handle reconnection after snapshot restoration."""
        if not self.adapter.computer:
            return

        if hasattr(self.adapter.computer, '_interface') and self.adapter.computer._interface:
            logger.info(f"Reconnecting interface after restoring {container_name}")
            try:
                await self.adapter.computer._interface.wait_for_ready()
                logger.info("Interface reconnected successfully")
            except Exception as e:
                logger.warning(f"Could not reconnect interface: {e}")