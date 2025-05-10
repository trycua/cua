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
        # PyLume returns a VMStatus object, not a dictionary
        # Access ip_address as an attribute, not with .get()
        return vm.ip_address if vm else None