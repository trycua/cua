"""Lume runtime — macOS VMs via Apple Virtualization.framework (macOS hosts only).

Requires the Lume CLI running on the host (default port 7777).
"""

from __future__ import annotations

import asyncio
import logging
import subprocess

import httpx
from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo
from cua_sandbox.runtime.images import (
    LUME_API_PORT,
    LUME_PROVIDER_PORT,
    MACOS_SEQUOIA,
    MACOS_VERSION_IMAGES,
)

logger = logging.getLogger(__name__)


def _has_lume() -> bool:
    try:
        subprocess.run(["lume", "--version"], capture_output=True, check=True)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


class LumeRuntime(Runtime):
    """Runs macOS VMs via the Lume CLI / API."""

    def __init__(
        self,
        *,
        lume_host: str = "localhost",
        lume_port: int = LUME_PROVIDER_PORT,
        api_port: int = LUME_API_PORT,
    ):
        self.lume_host = lume_host
        self.lume_port = lume_port
        self.api_port = api_port

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if not _has_lume():
            raise RuntimeError(
                "Lume CLI is not installed. "
                "Install from https://github.com/trycua/cua/tree/main/libs/lume"
            )

        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.get(f"{lume_url}/lume/vms/{name}")
            vm = resp.json()

            if vm.get("status") == "running":
                logger.info(f"Lume VM {name} already running")
            else:
                oci_ref = (
                    image._registry
                    or MACOS_VERSION_IMAGES.get(image.version or "")
                    or MACOS_SEQUOIA
                )
                logger.info(f"Pulling {oci_ref} via Lume...")
                # Lume's pull API expects image="name:tag" with registry/organization
                # passed separately — passing a full ref causes double-prefixing of the org.
                pull_payload: dict = {"image": oci_ref, "name": name}
                parts = oci_ref.split("/")
                if len(parts) >= 3 and "." in parts[0]:
                    pull_payload = {
                        "image": "/".join(parts[2:]),
                        "name": name,
                        "registry": parts[0],
                        "organization": parts[1],
                    }
                try:
                    await client.post(
                        f"{lume_url}/lume/pull",
                        json=pull_payload,
                        timeout=600,
                    )
                except httpx.ReadError:
                    # Lume closes the connection when pull starts asynchronously.
                    pass
                logger.info(f"Running Lume VM {name}...")
                await client.post(
                    f"{lume_url}/lume/vms/{name}/run",
                    json={
                        "noDisplay": opts.get("no_display", True),
                        "sharedDirectories": opts.get("shared_directories", []),
                    },
                    timeout=120,
                )

            ip = await self._wait_for_ip(name, lume_url)

        info = RuntimeInfo(host=ip, api_port=self.api_port, name=name)
        await self.is_ready(info)
        return info

    async def _wait_for_ip(self, name: str, lume_url: str, timeout: float = 3600) -> str:
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=10) as client:
            while asyncio.get_event_loop().time() < deadline:
                resp = await client.get(f"{lume_url}/lume/vms/{name}")
                data = resp.json()
                ip = data.get("ip_address") or data.get("ipAddress")
                if ip and ip != "unknown" and not ip.startswith("0.0.0.0"):
                    return ip
                await asyncio.sleep(3)
        raise TimeoutError(f"Lume VM {name} did not get an IP within {timeout}s")

    async def stop(self, name: str) -> None:
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=30) as client:
            await client.post(f"{lume_url}/lume/vms/{name}/stop")

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"Lume VM {info.name} computer-server is ready")
                        return True
                except (
                    httpx.ConnectError,
                    httpx.ReadTimeout,
                    httpx.ConnectTimeout,
                    httpx.ReadError,
                ):
                    pass
                await asyncio.sleep(3)
        raise TimeoutError(f"Lume VM {info.name} computer-server not ready after {timeout}s")
