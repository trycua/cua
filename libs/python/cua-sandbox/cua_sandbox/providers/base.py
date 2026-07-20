"""Provider lifecycle contracts for remotely managed sandboxes."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Optional, Protocol

if TYPE_CHECKING:
    from cua_sandbox.image import Image
    from cua_sandbox.transport.base import Transport


@dataclass
class ProvisionedSandbox:
    """Transport and opaque lifecycle handle returned by a provider."""

    name: str
    transport: "Transport"
    handle: Any


class SandboxProvider(Protocol):
    """Lifecycle operations required by Sandbox.create() and destroy()."""

    async def create(
        self,
        image: "Image",
        *,
        name: str,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
    ) -> ProvisionedSandbox: ...

    async def destroy(self, handle: Any) -> None: ...
