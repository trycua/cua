"""
Query and listing operations for snapshot providers.
"""

from typing import List, Dict, Any, Optional
import logging

logger = logging.getLogger(__name__)


class QueryOperations:
    """
    Handles query and listing operations for snapshots.
    """

    def __init__(self, adapter):
        """
        Initialize query operations.

        Args:
            adapter: Parent ProviderAdapter instance
        """
        self.adapter = adapter

    async def list_snapshots(self, container_name: str) -> List[Dict[str, Any]]:
        """
        List available snapshots through the provider.

        Args:
            container_name: Name of the container

        Returns:
            List of snapshot dictionaries
        """
        if not await self.adapter.validate_provider():
            logger.warning("Provider not available for listing snapshots")
            return []

        try:
            if self.adapter._provider is None:
                return []

            snapshots = await self.adapter._provider.list_snapshots(container_name)
            return snapshots if snapshots else []

        except NotImplementedError:
            logger.warning("Provider does not implement list_snapshots")
            return []
        except Exception as e:
            logger.error(f"Provider error listing snapshots: {e}")
            return []

    async def get_snapshot_info(self, container_name: str, snapshot_id: str) -> Optional[Dict[str, Any]]:
        """
        Get information about a specific snapshot.

        Args:
            container_name: Name of the container
            snapshot_id: ID of the snapshot

        Returns:
            Snapshot information or None if not found
        """
        snapshots = await self.list_snapshots(container_name)
        for snapshot in snapshots:
            if snapshot.get("id") == snapshot_id:
                return snapshot
        return None

    def get_provider_info(self) -> Dict[str, Any]:
        """
        Get information about the current provider.

        Returns:
            Dictionary with provider information
        """
        if not self.adapter._provider:
            return {"name": "none", "supports_snapshots": False}

        provider_name = type(self.adapter._provider).__name__
        return {
            "name": provider_name,
            "supports_snapshots": self.adapter._validated,
            "has_computer": self.adapter.computer is not None
        }