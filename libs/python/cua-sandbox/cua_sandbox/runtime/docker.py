"""Docker runtime — spins up containers with computer-server."""

from __future__ import annotations

import asyncio
import logging
import subprocess
from typing import Optional

import httpx
from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo
from cua_sandbox.runtime.images import (
    DEFAULT_API_PORT,
    DEFAULT_VNC_PORT,
    internal_ports,
    resolve_image,
)

logger = logging.getLogger(__name__)


def _has_docker() -> bool:
    try:
        subprocess.run(["docker", "info"], capture_output=True, check=True, timeout=10)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


def _has_kvm() -> bool:
    """Check if /dev/kvm is available (Linux/WSL2 only)."""
    import platform

    if platform.system() != "Linux":
        return False
    from pathlib import Path

    return Path("/dev/kvm").exists()


class DockerRuntime(Runtime):
    """Runs containers via ``docker run``."""

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
    ):
        self.api_port = api_port
        self.vnc_port = vnc_port
        self.ephemeral = ephemeral
        self.volumes = volumes or []
        self.environment = environment or {}
        self.devices = devices or []
        self.platform = platform
        self.privileged = privileged
        self.stop_timeout = stop_timeout

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if not _has_docker():
            raise RuntimeError("Docker is not installed or not running")

        docker_image = resolve_image(image.os_type, image._registry)
        internal_api, internal_vnc = internal_ports(docker_image)
        extra_flags: list[str] = []

        if "qemu" in docker_image:
            extra_flags += ["--cap-add", "NET_ADMIN"]

        # Volume mounts
        for v in self.volumes:
            extra_flags += ["-v", v]

        # Environment variables
        for k, val in self.environment.items():
            extra_flags += ["-e", f"{k}={val}"]

        # Devices (e.g. /dev/kvm)
        for d in self.devices:
            extra_flags += ["--device", d]

        # Platform (e.g. linux/amd64)
        if self.platform:
            extra_flags += ["--platform", self.platform]

        # Privileged mode
        if self.privileged:
            extra_flags.append("--privileged")

        # Stop timeout
        extra_flags += ["--stop-timeout", str(self.stop_timeout)]

        # Remove existing container with same name
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)

        cmd = [
            "docker",
            "run",
            "-d",
            "--name",
            name,
            "-p",
            f"{self.api_port}:{internal_api}",
            "-p",
            f"{self.vnc_port}:{internal_vnc}",
            *extra_flags,
            docker_image,
        ]
        logger.info(f"Starting container: {' '.join(cmd)}")
        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError(f"docker run failed: {result.stderr}")

        container_id = result.stdout.strip()[:12]
        info = RuntimeInfo(
            host="localhost",
            api_port=self.api_port,
            vnc_port=self.vnc_port,
            container_id=container_id,
            name=name,
        )
        await self.is_ready(info)
        return info

    async def stop(self, name: str) -> None:
        subprocess.run(["docker", "stop", name], capture_output=True)
        if self.ephemeral:
            subprocess.run(["docker", "rm", name], capture_output=True)

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"Container {info.name} is ready")
                        return True
                except (
                    httpx.ConnectError,
                    httpx.ReadTimeout,
                    httpx.RemoteProtocolError,
                    httpx.ConnectTimeout,
                    httpx.ReadError,
                ):
                    pass
                await asyncio.sleep(2)
        raise TimeoutError(f"Container {info.name} not ready after {timeout}s")
