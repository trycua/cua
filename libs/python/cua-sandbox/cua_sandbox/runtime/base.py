"""Abstract runtime — starts a VM or container from an Image spec and returns connection info."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Optional

from cua_sandbox.image import Image


@dataclass
class RuntimeInfo:
    """Connection info returned after a runtime spins up."""

    host: str
    api_port: int
    vnc_port: Optional[int] = None
    api_key: Optional[str] = None
    container_id: Optional[str] = None
    name: Optional[str] = None
    qmp_port: Optional[int] = None  # Set when QMP transport should be used
    environment: Optional[str] = None  # OS type hint for QMP transport
    agent_type: Optional[str] = None  # e.g. "osworld" for OSWorld Flask server
    guest_server_port: int = 8000  # Port the guest server listens on
    ssh_port: Optional[int] = None  # SSH port on the guest
    ssh_username: Optional[str] = None
    ssh_password: Optional[str] = None
    ssh_key_filename: Optional[str] = None
    vnc_host: Optional[str] = None  # VNC host (if different from host, e.g. localhost for Tart)
    vnc_password: Optional[str] = None
    grpc_port: Optional[int] = (
        None  # Set when gRPC transport should be used (emulator gRPC service)
    )


class Runtime(ABC):
    """Base class for local runtimes that create VMs/containers from an Image."""

    @abstractmethod
    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        """Start a VM/container and return connection info."""

    @abstractmethod
    async def stop(self, name: str) -> None:
        """Stop and clean up a VM/container."""

    @abstractmethod
    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        """Wait until the computer-server inside is reachable."""
