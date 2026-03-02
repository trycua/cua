"""Factory for creating computer interfaces."""

from typing import Literal, Optional

from .base import BaseComputerInterface

OSType = Literal["macos", "linux", "windows", "android"]


class InterfaceFactory:
    """Factory for creating OS-specific computer interfaces."""

    @staticmethod
    def create_interface_for_os(
        os: OSType,
        ip_address: str,
        api_port: Optional[int] = None,
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
    ) -> BaseComputerInterface:
        """Create an interface for the specified OS.

        Args:
            os: Operating system type ('macos', 'linux', or 'windows')
            ip_address: IP address of the computer to control
            api_port: Optional API port of the computer to control
            api_key: Optional API key for cloud authentication
            vm_name: Optional VM name for cloud authentication

        Returns:
            BaseComputerInterface: The appropriate interface for the OS

        Raises:
            ValueError: If the OS type is not supported
        """

        if os == "macos":
            from .macos import MacOSComputerInterface

            return MacOSComputerInterface(
                ip_address, api_key=api_key, vm_name=vm_name, api_port=api_port
            )
        elif os == "linux":
            from .linux import LinuxComputerInterface

            return LinuxComputerInterface(
                ip_address, api_key=api_key, vm_name=vm_name, api_port=api_port
            )
        elif os == "windows":
            from .windows import WindowsComputerInterface

            return WindowsComputerInterface(
                ip_address, api_key=api_key, vm_name=vm_name, api_port=api_port
            )
        elif os == "android":
            from .android import AndroidComputerInterface

            return AndroidComputerInterface(
                ip_address, api_key=api_key, vm_name=vm_name, api_port=api_port
            )
        else:
            raise ValueError(f"Unsupported OS type: {os}")
