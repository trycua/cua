"""Base provider interface for VM backends."""

import abc
from enum import Enum
from typing import Dict, List, Optional, Any, AsyncContextManager


class VMProviderType(str, Enum):
    """Enum of supported VM provider types."""
    LUME = "lume"
    QEMU = "qemu"
    CLOUD = "cloud"
    UNKNOWN = "unknown"


class BaseVMProvider(AsyncContextManager):
    """Base interface for VM providers.
    
    All VM provider implementations must implement this interface.
    """
    
    @property
    @abc.abstractmethod
    def provider_type(self) -> VMProviderType:
        """Get the provider type."""
        pass
        
    @abc.abstractmethod
    async def get_vm(self, name: str) -> Dict[str, Any]:
        """Get VM information by name."""
        pass
        
    @abc.abstractmethod
    async def list_vms(self) -> List[Dict[str, Any]]:
        """List all available VMs."""
        pass
        
    @abc.abstractmethod
    async def run_vm(self, name: str, run_opts: Dict[str, Any]) -> Dict[str, Any]:
        """Run a VM with the given options."""
        pass
        
    @abc.abstractmethod
    async def stop_vm(self, name: str) -> Dict[str, Any]:
        """Stop a running VM."""
        pass
        
    @abc.abstractmethod
    async def update_vm(self, name: str, update_opts: Dict[str, Any]) -> Dict[str, Any]:
        """Update VM configuration."""
        pass
