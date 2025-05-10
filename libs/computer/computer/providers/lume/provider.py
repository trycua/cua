"""Lume VM provider implementation."""

import logging
from typing import Dict, List, Optional, Any, Tuple, TypeVar, Type

# Only import pylume when this module is actually used
try:
    from pylume import PyLume
    from pylume.models import VMRunOpts, VMUpdateOpts, ImageRef, SharedDirectory, VMStatus
    HAS_PYLUME = True
except ImportError:
    HAS_PYLUME = False
    # Create dummy classes for type checking
    class PyLume:
        pass
    class VMRunOpts:
        pass
    class VMUpdateOpts:
        pass
    class ImageRef:
        pass
    class SharedDirectory:
        pass
    class VMStatus:
        pass

from ..base import BaseVMProvider, VMProviderType

logger = logging.getLogger(__name__)


class LumeProvider(BaseVMProvider):
    """Lume VM provider implementation using pylume."""
    
    def __init__(
        self, 
        port: Optional[int] = None,
        host: str = "localhost",
        bin_path: Optional[str] = None,
        storage_path: Optional[str] = None,
        verbose: bool = False,
        **kwargs
    ):
        """Initialize the Lume provider.
        
        Args:
            port: Optional port to use for the PyLume server
            host: Host to use for PyLume connections
            bin_path: Optional path to the Lume binary
            storage_path: Optional path to store VM data
            verbose: Enable verbose logging
        """
        if not HAS_PYLUME:
            raise ImportError(
                "The pylume package is required for LumeProvider. "
                "Please install it with 'pip install cua-computer[lume]'"
            )
            
        # PyLume doesn't accept bin_path or storage_path parameters
        # Convert verbose to debug parameter for PyLume
        self._pylume = PyLume(
            port=port,
            host=host,
            debug=verbose,
            **kwargs
        )
        # Store these for reference, even though PyLume doesn't use them directly
        self._bin_path = bin_path
        self._storage_path = storage_path
        self._context = None
        
    @property
    def provider_type(self) -> VMProviderType:
        """Get the provider type."""
        return VMProviderType.LUME
        
    async def __aenter__(self):
        """Enter async context manager."""
        self._context = await self._pylume.__aenter__()
        return self
        
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Exit async context manager."""
        if self._context:
            await self._pylume.__aexit__(exc_type, exc_val, exc_tb)
            self._context = None
            
    async def get_vm(self, name: str) -> VMStatus:
        """Get VM information by name."""
        # PyLume get_vm returns a VMStatus object, not a dictionary
        return await self._pylume.get_vm(name)
        
    async def list_vms(self) -> List[Dict[str, Any]]:
        """List all available VMs."""
        return await self._pylume.list_vms()
        
    async def run_vm(self, name: str, run_opts: Dict[str, Any]) -> Dict[str, Any]:
        """Run a VM with the given options."""
        # Convert dict to VMRunOpts if needed
        if isinstance(run_opts, dict):
            run_opts = VMRunOpts(**run_opts)
        return await self._pylume.run_vm(name, run_opts)
        
    async def stop_vm(self, name: str) -> Dict[str, Any]:
        """Stop a running VM."""
        return await self._pylume.stop_vm(name)
        
    async def update_vm(self, name: str, update_opts: Dict[str, Any]) -> Dict[str, Any]:
        """Update VM configuration."""
        # Convert dict to VMUpdateOpts if needed
        if isinstance(update_opts, dict):
            update_opts = VMUpdateOpts(**update_opts)
        return await self._pylume.update_vm(name, update_opts)
    
    # Pylume-specific helper methods
    def get_pylume_instance(self) -> PyLume:
        """Get the underlying PyLume instance."""
        return self._pylume
        
    # Helper methods for converting between PyLume and generic types
    @staticmethod
    def create_vm_run_opts(**kwargs) -> VMRunOpts:
        """Create VMRunOpts from kwargs."""
        return VMRunOpts(**kwargs)
        
    @staticmethod
    def create_vm_update_opts(**kwargs) -> VMUpdateOpts:
        """Create VMUpdateOpts from kwargs."""
        return VMUpdateOpts(**kwargs)
