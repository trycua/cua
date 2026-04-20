from typing import Optional

from .generic import GenericComputerInterface


class AndroidComputerInterface(GenericComputerInterface):
    """Interface for Android."""

    def __init__(
        self,
        ip_address: str,
        username: str = "lume",
        password: str = "lume",
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
        api_port: Optional[int] = None,
        proxy_base_url: Optional[str] = None,
    ):
        super().__init__(
            ip_address, username, password, api_key, vm_name, "computer.interface.android", api_port, proxy_base_url
        )
