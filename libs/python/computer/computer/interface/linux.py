from typing import Dict, Optional

from .generic import GenericComputerInterface


class LinuxComputerInterface(GenericComputerInterface):
    """Interface for Linux."""

    def __init__(
        self,
        ip_address: str,
        username: str = "lume",
        password: str = "lume",
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
        api_port: Optional[int] = None,
        api_base_url: Optional[str] = None,
        api_headers: Optional[Dict[str, str]] = None,
    ):
        super().__init__(
            ip_address,
            username,
            password,
            api_key,
            vm_name,
            "computer.interface.linux",
            api_port,
            api_base_url=api_base_url,
            api_headers=api_headers,
        )
