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
        # If already validated and provider is available, return cached result
        if self._validated and self._provider is not None:
            return True

        if not self.computer:
            logger.warning("No computer instance available")
            return False

        if not hasattr(self.computer, 'config'):
            logger.warning("Computer does not have a config attribute")
            return False

        # Check if vm_provider exists and is not None
        if not hasattr(self.computer.config, 'vm_provider') or self.computer.config.vm_provider is None:
            # This is expected during initialization - provider is set later during run()
            # Reset validation to allow retry later
            self._validated = False
            self._provider = None
            logger.debug("VM provider not yet initialized - will retry when needed")
            return False

        provider = self.computer.config.vm_provider

        # Debug information
        logger.info(f"Validating provider: {type(provider).__name__}")
        logger.info(f"Provider type: {type(provider)}")
        logger.info(f"Provider MRO: {[cls.__name__ for cls in type(provider).__mro__]}")

        # Show all available methods
        all_methods = dir(provider)
        snapshot_methods = [m for m in all_methods if 'snapshot' in m.lower()]
        logger.info(f"Available snapshot-related methods: {snapshot_methods}")

        # Check if provider is actually the DockerProvider instance
        logger.info(f"Provider instance id: {id(provider)}")
        logger.info(f"Provider __class__.__module__: {provider.__class__.__module__}")

        required_methods = ['create_snapshot', 'restore_snapshot', 'list_snapshots', 'delete_snapshot']
        for method in required_methods:
            has_method = hasattr(provider, method)
            logger.info(f"Provider {type(provider).__name__} hasattr({method}) = {has_method}")

            # Try alternative check
            try:
                method_exists = method in dir(provider)
                logger.info(f"Provider {type(provider).__name__} '{method}' in dir() = {method_exists}")
            except Exception as e:
                logger.error(f"Error checking dir for {method}: {e}")

            if not has_method:
                logger.warning(f"Provider {type(provider).__name__} missing method: {method}")
                logger.debug(f"Available methods: {[m for m in dir(provider) if not m.startswith('_')]}")
                return False
            else:
                # Check if method is actually implemented (not just raising NotImplementedError)
                method_obj = getattr(provider, method)
                if callable(method_obj):
                    logger.debug(f"Provider {type(provider).__name__} has method: {method}")
                else:
                    logger.warning(f"Provider {type(provider).__name__} method {method} is not callable")
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