"""Factory for creating computer interfaces."""

from typing import Dict, Literal, Optional

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
        api_base_url: Optional[str] = None,
        api_headers: Optional[Dict[str, str]] = None,
    ) -> BaseComputerInterface:
        """Create an interface for the specified OS.

        Args:
            os: Operating system type ('macos', 'linux', or 'windows')
            ip_address: IP address of the computer to control
            api_port: Optional API port of the computer to control
            api_key: Optional API key for cloud authentication
            vm_name: Optional VM name for cloud authentication
            api_base_url: Optional explicit base URL for the computer-server, e.g. when
                it sits behind an authenticated, path-prefixed reverse proxy. When set,
                REST/WebSocket URIs are derived from it instead of ip_address + port.
            api_headers: Optional extra HTTP headers (e.g. ``Authorization: Bearer ...``)
                sent on every REST request and the WebSocket upgrade.

        Returns:
            BaseComputerInterface: The appropriate interface for the OS

        Raises:
            ValueError: If the OS type is not supported
        """

        common_kwargs = dict(
            api_key=api_key,
            vm_name=vm_name,
            api_port=api_port,
            api_base_url=api_base_url,
            api_headers=api_headers,
        )

        if os == "macos":
            from .macos import MacOSComputerInterface

            return MacOSComputerInterface(ip_address, **common_kwargs)
        elif os == "linux":
            from .linux import LinuxComputerInterface

            return LinuxComputerInterface(ip_address, **common_kwargs)
        elif os == "windows":
            from .windows import WindowsComputerInterface

            return WindowsComputerInterface(ip_address, **common_kwargs)
        elif os == "android":
            from .android import AndroidComputerInterface

            return AndroidComputerInterface(ip_address, **common_kwargs)
        else:
            raise ValueError(f"Unsupported OS type: {os}")
