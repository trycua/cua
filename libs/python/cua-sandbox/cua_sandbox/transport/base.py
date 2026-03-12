"""Abstract transport protocol.

A Transport moves commands and data between the sandbox client and the
underlying computer (local host, WebSocket to computer-server, or cloud API).
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any, Dict, Optional


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
