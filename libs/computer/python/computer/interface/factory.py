"""Factory for creating computer interfaces."""

from typing import Literal, Optional
from .base import BaseComputerInterface

class InterfaceFactory:
    """Factory for creating OS-specific computer interfaces."""
    
    @staticmethod
    def create_interface_for_os(
        os: Literal['macos', 'linux', 'windows'],
        ip_address: str,
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None
    ) -> BaseComputerInterface:
        """Create an interface for the specified OS.
        
        Args:
            os: Operating system type ('macos', 'linux', or 'windows')
            ip_address: IP address of the computer to control
            api_key: Optional API key for cloud authentication
            vm_name: Optional VM name for cloud authentication
            
        Returns:
            BaseComputerInterface: The appropriate interface for the OS
            
        Raises:
            ValueError: If the OS type is not supported
        """
        # Import implementations here to avoid circular imports
        from .macos import MacOSComputerInterface
        from .linux import LinuxComputerInterface
        from .windows import WindowsComputerInterface
        
        if os == 'macos':
            return MacOSComputerInterface(ip_address, api_key=api_key, vm_name=vm_name)
        elif os == 'linux':
            return LinuxComputerInterface(ip_address, api_key=api_key, vm_name=vm_name)
        elif os == 'windows':
            return WindowsComputerInterface(ip_address, api_key=api_key, vm_name=vm_name)
        else:
            raise ValueError(f"Unsupported OS type: {os}")
