"""Factory for creating VM providers."""

import logging
from typing import Dict, Optional, Any, Type, Union

from .base import BaseVMProvider, VMProviderType

logger = logging.getLogger(__name__)


class VMProviderFactory:
    """Factory for creating VM providers based on provider type."""

    @staticmethod
    def create_provider(
        provider_type: Union[str, VMProviderType],
        **kwargs
    ) -> BaseVMProvider:
        """Create a VM provider of the specified type.
        
        Args:
            provider_type: Type of VM provider to create
            **kwargs: Additional arguments to pass to the provider constructor
            
        Returns:
            An instance of the requested VM provider
            
        Raises:
            ImportError: If the required dependencies for the provider are not installed
            ValueError: If the provider type is not supported
        """
        # Convert string to enum if needed
        if isinstance(provider_type, str):
            try:
                provider_type = VMProviderType(provider_type.lower())
            except ValueError:
                provider_type = VMProviderType.UNKNOWN
        
        if provider_type == VMProviderType.LUME:
            try:
                from .lume import LumeProvider, HAS_LUME
                if not HAS_LUME:
                    raise ImportError(
                        "The pylume package is required for LumeProvider. "
                        "Please install it with 'pip install cua-computer[lume]'"
                    )
                return LumeProvider(**kwargs)
            except ImportError as e:
                logger.error(f"Failed to import LumeProvider: {e}")
                raise ImportError(
                    "The pylume package is required for LumeProvider. "
                    "Please install it with 'pip install cua-computer[lume]'"
                ) from e
        elif provider_type == VMProviderType.QEMU:
            try:
                from .qemu import QEMUProvider
                return QEMUProvider(**kwargs)
            except ImportError as e:
                logger.error(f"Failed to import QEMUProvider: {e}")
                raise ImportError(
                    "The qemu package is required for QEMUProvider. "
                    "Please install it with 'pip install cua-computer[qemu]'"
                ) from e
        elif provider_type == VMProviderType.CLOUD:
            try:
                from .cloud import CloudProvider
                return CloudProvider(**kwargs)
            except ImportError as e:
                logger.error(f"Failed to import CloudProvider: {e}")
                raise ImportError(
                    "Cloud provider dependencies are required for CloudProvider. "
                    "Please install them with 'pip install cua-computer[cloud]'"
                ) from e
        else:
            raise ValueError(f"Unsupported provider type: {provider_type}")
