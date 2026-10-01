"""Docker runtime — OCI containers through the cua SDK (gVisor ``runsc`` when available).

The SDK's container backend (cua-vmm) runs the image with ``runsc`` when the
engine has it (and can install it into Colima/Docker Desktop), publishes the
image's ports and cua-spacesd's 3211, and records the sandbox in
``~/.cua/sandboxes``. Readiness is daemon-agnostic: the container is running.
"""

from __future__ import annotations

import logging
import subprocess
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from cua_sandbox.image import Image

from cua_sandbox.image import Image
from cua_sandbox.runtime.base import RuntimeInfo
from cua_sandbox.runtime.images import DEFAULT_API_PORT, DEFAULT_VNC_PORT
from cua_sandbox.runtime.native import NativeRuntime

logger = logging.getLogger(__name__)


def _find_free_port(start: int = 8000, end: int = 9000) -> int:
    """Return a free TCP port. Uses OS assignment (port 0) to avoid races."""
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("", 0))
        return s.getsockname()[1]


def _docker_bin() -> str:
    """Return the resolved path to the docker CLI binary.

    Probes common install locations in addition to PATH — SSH sessions often
    have a stripped PATH that omits /usr/local/bin (e.g. OrbStack on macOS).
    Raises RuntimeError if docker is not found.
    """
    import os
    import shutil

    candidates = [
        "docker",
        "/usr/local/bin/docker",
        "/opt/homebrew/bin/docker",
        "/usr/bin/docker",
        os.path.expanduser("~/.docker/bin/docker"),
    ]
    for candidate in candidates:
        resolved = shutil.which(candidate) or candidate
        try:
            subprocess.run([resolved, "info"], capture_output=True, check=True, timeout=10)
            return resolved
        except (subprocess.SubprocessError, FileNotFoundError, OSError):
            continue
    raise RuntimeError(
        "Docker not found. Install from https://docker.com or ensure the docker CLI is on PATH."
    )


def _has_docker() -> bool:
    """Return True if a Docker daemon is reachable."""
    try:
        _docker_bin()
        return True
    except RuntimeError:
        return False


def _has_kvm() -> bool:
    """Check if /dev/kvm is available (Linux/WSL2 only)."""
    import platform

    if platform.system() != "Linux":
        return False
    from pathlib import Path

    return Path("/dev/kvm").exists()


class DockerRuntime(NativeRuntime):
    """Runs OCI container images through the SDK's container backend.

    ``api_port``/``vnc_port`` are accepted for compatibility; the SDK picks
    free host ports and :attr:`Sandbox.exposed_ports` reports them.
    ``volumes``, ``devices``, ``platform`` and ``privileged`` are not
    supported by the sandboxed (gVisor) backend and are rejected.
    """

    runtime_type = "container"
    env_ready_timeout = 60.0
    delivers_guest_env = True

    def __init__(
        self,
        *,
        api_port: int = DEFAULT_API_PORT,
        vnc_port: int = DEFAULT_VNC_PORT,
        ephemeral: bool = True,
        volumes: Optional[list[str]] = None,
        environment: Optional[dict[str, str]] = None,
        devices: Optional[list[str]] = None,
        platform: Optional[str] = None,
        privileged: bool = False,
        stop_timeout: int = 120,
        cpus: Optional[int] = None,
        memory_mb: Optional[int] = None,
        server_port: Optional[int] = None,
    ):
        unsupported = [
            flag
            for flag, value in (
                ("volumes", volumes),
                ("devices", devices),
                ("platform", platform),
                ("privileged", privileged),
            )
            if value
        ]
        if unsupported:
            raise ValueError(
                f"DockerRuntime no longer supports {', '.join(unsupported)}: containers run "
                "sandboxed through the cua SDK (gVisor when available). Bake files into the "
                "image with Image.copy()/Image.run() instead."
            )
        super().__init__(
            ephemeral=ephemeral,
            cpus=cpus,
            memory_mb=memory_mb,
            server_port=server_port,
            environment=environment,
        )
        self.api_port = api_port
        self.vnc_port = vnc_port
        self.stop_timeout = stop_timeout

    async def _image_ref(self, image: Image, name: str, **opts) -> str:
        from cua_sandbox.image import cloud_registry_image

        ref = cloud_registry_image(image)
        if ref is None:
            raise ValueError(
                f"no image for {image.os_type}/{image.distro} {image.version}; use "
                "Image.linux() (ghcr.io/trycua/linux) or Image.from_registry(ref)"
            )
        return "container:" + ref

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if image.os_type not in (None, "linux"):
            raise NotImplementedError(
                f"DockerRuntime runs Linux containers; {image.os_type} images need a VM "
                "runtime (QEMURuntime, LumeRuntime)"
            )
        return await super().start(image, name, **opts)
