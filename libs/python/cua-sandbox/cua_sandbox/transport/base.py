"""Abstract transport protocol.

A Transport moves commands and data between the sandbox client and the
underlying computer (local host, WebSocket to computer-server, or cloud API).
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any, Dict

if TYPE_CHECKING:
    from cua_sandbox.interfaces.tunnel import TunnelInfo


def convert_screenshot(png_bytes: bytes, format: str, quality: int) -> bytes:
    """Convert raw PNG bytes to the requested format.

    Args:
        png_bytes: Raw PNG image bytes.
        format: "png", "jpeg", or "jpg".
        quality: JPEG quality (1-95), ignored for PNG.
    """
    fmt = format.lower()
    if fmt in ("jpeg", "jpg"):
        from io import BytesIO

        from PIL import Image as PILImage

        img = PILImage.open(BytesIO(png_bytes)).convert("RGB")
        buf = BytesIO()
        img.save(buf, format="JPEG", quality=quality)
        return buf.getvalue()
    return png_bytes


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
    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        """Capture a screenshot and return raw image bytes.

        Args:
            format: "png" (lossless, default) or "jpeg" (lossy, ~5-10x smaller).
            quality: JPEG quality 1-95, ignored for PNG.
        """

    @abstractmethod
    async def get_screen_size(self) -> Dict[str, int]:
        """Return {"width": ..., "height": ...}."""

    @abstractmethod
    async def get_environment(self) -> str:
        """Return 'windows', 'mac', 'linux', or 'browser'."""

    async def forward_tunnel(self, sandbox_port: int) -> "TunnelInfo":
        """Forward *sandbox_port* to an available host port and return info.

        Subclasses that support tunnelling must override this method.
        The returned :class:`~cua_sandbox.interfaces.tunnel.TunnelInfo` must
        have ``host`` and ``port`` set to the host-side address.
        """
        raise NotImplementedError(
            f"{type(self).__name__} does not support port forwarding. "
            "Supported transports: ADBTransport, GRPCEmulatorTransport, SSHTransport."
        )

    async def close_tunnel(self, info: "TunnelInfo") -> None:
        """Release a previously forwarded port.  No-op by default."""

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
