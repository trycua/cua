"""
Core provider adapter for snapshot operations.
"""

from typing import Optional, Dict, Any, List
import logging

# Import within methods to avoid circular imports

logger = logging.getLogger(__name__)


class ProviderAdapter:
    """
    Adapter for interacting with VM/container providers.
    """

    def __init__(self, computer: Optional[Any] = None):
        """
        Initialize the provider adapter.

        Args:
            computer: Computer instance with VM provider configuration
        """
        from .snapshot_ops import SnapshotOperations
        from .query_ops import QueryOperations

        self.computer = computer
        self._provider = None
        self._validated = False

        self.snapshot_ops = SnapshotOperations(self)
        self.query_ops = QueryOperations(self)

    async def validate_provider(self) -> bool:
        """
        Validate that the provider supports snapshot operations.

        Returns:
            True if provider supports snapshots, False otherwise
        """
        if self._validated:
            return self._provider is not None

        if not self.computer:
            logger.warning("No computer instance available")
            return False

        if not hasattr(self.computer, 'config') or not hasattr(self.computer.config, 'vm_provider'):
            logger.warning("Computer does not have a configured VM provider")
            return False

        provider = self.computer.config.vm_provider

        required_methods = ['create_snapshot', 'restore_snapshot', 'list_snapshots', 'delete_snapshot']
        for method in required_methods:
            if not hasattr(provider, method):
                logger.warning(f"Provider {type(provider).__name__} missing method: {method}")
                return False

        self._provider = provider
        self._validated = True
        logger.debug(f"Provider {type(provider).__name__} validated for snapshot operations")
        return True

    async def create_snapshot(self, container_name: str, snapshot_name: str,
                            metadata: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        """Create a snapshot through the provider."""
        return await self.snapshot_ops.create(container_name, snapshot_name, metadata)

    async def restore_snapshot(self, container_name: str, snapshot_id: str) -> Dict[str, Any]:
        """Restore a snapshot through the provider."""
        return await self.snapshot_ops.restore(container_name, snapshot_id)

    async def delete_snapshot(self, snapshot_id: str) -> Dict[str, Any]:
        """Delete a snapshot through the provider."""
        return await self.snapshot_ops.delete(snapshot_id)

    async def list_snapshots(self, container_name: str) -> List[Dict[str, Any]]:
        """List available snapshots through the provider."""
        return await self.query_ops.list_snapshots(container_name)

    def get_provider_info(self) -> Dict[str, Any]:
        """Get information about the current provider."""
        return self.query_ops.get_provider_info()

    def reset_validation(self) -> None:
        """Reset the validation state to force revalidation."""
        self._validated = False
        self._provider = None
        logger.debug("Provider validation reset")