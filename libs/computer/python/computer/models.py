"""Models for computer configuration."""

from dataclasses import dataclass
from typing import Optional, Any, Dict

# Import base provider interface
from .providers.base import BaseVMProvider

@dataclass
class Display:
    """Display configuration."""
    width: int
    height: int

@dataclass
class Image:
    """VM image configuration."""
    image: str
    tag: str
    name: str

@dataclass
class Computer:
    """Computer configuration."""
    image: str
    tag: str
    name: str
    display: Display
    memory: str
    cpu: str
    vm_provider: Optional[BaseVMProvider] = None
    
    # @property   # Remove the property decorator
    async def get_ip(self) -> Optional[str]:
        """Get the IP address of the VM."""
        if not self.vm_provider:
            return None
            
        vm = await self.vm_provider.get_vm(self.name)
        # Handle both object attribute and dictionary access for ip_address
        if vm:
            if isinstance(vm, dict):
                return vm.get("ip_address")
            else:
                # Access as attribute for object-based return values
                return getattr(vm, "ip_address", None)
        return None