"""Abstract transport protocol.

A Transport moves commands and data between the sandbox client and the
underlying computer (local host, WebSocket to computer-server, or cloud API).
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any, Dict


class Transport(ABC):
    """Base class for all transports."""

    @abstractmethod
    async def connect(self) -> None:
        """Establish the transport connection."""

    @abstractmethod
    async def disconnect(self) -> None:
        """Tear down the transport connection."""

    @abstractmethod
    async def send(self, action: str, **params: Any) -> Any:
        """Send a command and return the result."""

    @abstractmethod
    async def screenshot(self) -> bytes:
        """Capture a screenshot and return raw PNG bytes."""

    @abstractmethod
    async def get_screen_size(self) -> Dict[str, int]:
        """Return {"width": ..., "height": ...}."""

    @abstractmethod
    async def get_environment(self) -> str:
        """Return 'windows', 'mac', 'linux', or 'browser'."""

    async def get_display_url(self, *, share: bool = False) -> str:
        """Return a URL to view this sandbox's display.

        Args:
            share: If True, return a public link with embedded credentials
                   (cloud only). If False, return a direct connection URL
                   (localhost VNC for local runtimes; auth-gated URL for cloud).

        Raises NotImplementedError for transports that don't expose a display
        (e.g. HTTP, ADB).
        """
        raise NotImplementedError(f"{type(self).__name__} does not support get_display_url().")
