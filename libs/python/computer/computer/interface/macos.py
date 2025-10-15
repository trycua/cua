from .generic import GenericComputerInterface
from typing import Optional

class MacOSComputerInterface(GenericComputerInterface):
    """Interface for macOS."""

    def __init__(
        self,
        ip_address: str,
        username: str = "lume",
        password: str = "lume",
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
        port: Optional[int] = None,
    ):
        super().__init__(
            ip_address,
            username,
            password,
            api_key,
            vm_name,
            "computer.interface.macos",
            port,
        )

    async def diorama_cmd(self, action: str, arguments: Optional[dict] = None) -> dict:
        """Send a diorama command to the server (macOS only)."""
        return await self._send_command("diorama_cmd", {"action": action, "arguments": arguments or {}})
