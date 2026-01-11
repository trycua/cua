"""Factory for creating VM providers."""

import logging
from typing import Any, Dict, Optional, Type, Union

from .base import BaseVMProvider, VMProviderType

logger = logging.getLogger(__name__)


class VMProviderFactory:
    """Factory for creating VM providers based on provider type."""

    @staticmethod
    def create_provider(
        provider_type: Union[str, VMProviderType],
        provider_port: int = 7777,
        host: str = "localhost",
        bin_path: Optional[str] = None,
        storage: Optional[str] = None,
        shared_path: Optional[str] = None,
        image: Optional[str] = None,
        verbose: bool = False,
        ephemeral: bool = False,
        noVNC_port: Optional[int] = None,
        api_port: Optional[int] = None,
        **kwargs,
    ) -> BaseVMProvider:
        """Create a VM provider of the specified type.

        Args:
            provider_type: Type of VM provider to create
            provider_port: Port for the provider's API server
            host: Hostname for the API server
            bin_path: Path to provider binary if needed
            storage: Path for persistent VM storage
            shared_path: Path for shared folder between host and VM
            image: VM image to use (for Lumier provider)
            verbose: Enable verbose logging
            ephemeral: Use ephemeral (temporary) storage
            noVNC_port: Specific port for noVNC interface (for Lumier and Docker provider)
            api_port: Specific port for Computer API server (for Docker provider)

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
                from .lume import HAS_LUME, LumeProvider

                if not HAS_LUME:
                    raise ImportError(
                        "LumeProvider requires curl and the Lume CLI. "
                        "Please install Lume from https://github.com/trycua/cua/tree/main/libs/lume"
                    )
                return LumeProvider(
                    provider_port=provider_port,
                    host=host,
                    storage=storage,
                    verbose=verbose,
                    ephemeral=ephemeral,
                )
            except ImportError as e:
                logger.error(f"Failed to import LumeProvider: {e}")
                raise ImportError(
                    "LumeProvider requires curl and the Lume CLI. "
                    "Please install Lume from https://github.com/trycua/cua/tree/main/libs/lume"
                ) from e
        elif provider_type == VMProviderType.LUMIER:
            try:
                from .lumier import HAS_LUMIER, LumierProvider

                if not HAS_LUMIER:
                    raise ImportError(
                        "Docker is required for LumierProvider. "
                        "Please install Docker for Apple Silicon and Lume CLI before using this provider."
                    )
                return LumierProvider(
                    provider_port=provider_port,
                    host=host,
                    storage=storage,
                    shared_path=shared_path,
                    image=image or "macos-sequoia-cua:latest",
                    verbose=verbose,
                    ephemeral=ephemeral,
                    noVNC_port=noVNC_port,
                )
            except ImportError as e:
                logger.error(f"Failed to import LumierProvider: {e}")
                raise ImportError(
                    "Docker and Lume CLI are required for LumierProvider. "
                    "Please install Docker for Apple Silicon and run the Lume installer script."
                ) from e

        elif provider_type == VMProviderType.CLOUD:
            try:
                from .cloud import CloudProvider

                return CloudProvider(
                    verbose=verbose,
                    **kwargs,
                )
            except ImportError as e:
                logger.error(f"Failed to import CloudProvider: {e}")
                raise ImportError(
                    "The CloudProvider is not fully implemented yet. "
                    "Please use LUME or LUMIER provider instead."
                ) from e
        elif provider_type == VMProviderType.WINSANDBOX:
            try:
                from .winsandbox import HAS_WINSANDBOX, WinSandboxProvider

                if not HAS_WINSANDBOX:
                    raise ImportError(
                        "pywinsandbox is required for WinSandboxProvider. "
                        "Please install it with 'pip install -U git+https://github.com/karkason/pywinsandbox.git'"
                    )
                return WinSandboxProvider(
                    host=host,
                    storage=storage,
                    verbose=verbose,
                    ephemeral=ephemeral,
                    **kwargs,
                )
            except ImportError as e:
                logger.error(f"Failed to import WinSandboxProvider: {e}")
                raise ImportError(
                    "pywinsandbox is required for WinSandboxProvider. "
                    "Please install it with 'pip install -U git+https://github.com/karkason/pywinsandbox.git'"
                ) from e
        elif provider_type == VMProviderType.DOCKER:
            try:
                from .docker import HAS_DOCKER, DockerProvider

                if not HAS_DOCKER:
                    raise ImportError(
                        "Docker is required for DockerProvider. "
                        "Please install Docker and ensure it is running."
                    )
                return DockerProvider(
                    host=host,
                    storage=storage,
                    shared_path=shared_path,
                    image=image or "trycua/cua-ubuntu:latest",
                    verbose=verbose,
                    ephemeral=ephemeral,
                    vnc_port=noVNC_port,
                    api_port=api_port,
                )
            except ImportError as e:
                logger.error(f"Failed to import DockerProvider: {e}")
                raise ImportError(
                    "Docker is required for DockerProvider. "
                    "Please install Docker and ensure it is running."
                ) from e
        else:
            raise ValueError(f"Unsupported provider type: {provider_type}")
