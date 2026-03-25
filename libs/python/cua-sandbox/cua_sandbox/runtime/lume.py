"""Lume runtime — macOS VMs via Apple Virtualization.framework (macOS hosts only).

Requires the Lume CLI running on the host (default port 7777).
"""

from __future__ import annotations

import asyncio
import logging
import subprocess
from typing import TYPE_CHECKING

import httpx
from cua_sandbox.image import Image

if TYPE_CHECKING:
    pass
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
                # Use /lume/pull/start (async, returns 202 immediately) so we can
                # poll progress while the pull runs in the background.
                # Falls back to the synchronous /lume/pull if start returns 404.
                try:
                    start_resp = await client.post(
                        f"{lume_url}/lume/pull/start",
                        json=pull_payload,
                        timeout=30,
                    )
                except (httpx.ReadError, httpx.RemoteProtocolError):
                    # /pull/start not available in lume v0.3.x — fall back to sync /pull,
                    # then run the VM and return directly (bypassing the async poll below).
                    logger.info(
                        "Lume /pull/start unavailable (v0.3.x), falling back to synchronous pull..."
                    )
                    print("\rPulling macOS image...", end="", flush=True)
                    try:
                        sync_resp = await client.post(
                            f"{lume_url}/lume/pull",
                            json=pull_payload,
                            timeout=1800,
                        )
                        if sync_resp.status_code >= 400:
                            try:
                                detail = sync_resp.json()
                            except Exception:
                                detail = sync_resp.text
                            raise RuntimeError(f"Lume pull failed for '{name}': {detail}")
                    except (httpx.ReadError, httpx.RemoteProtocolError):
                        # Lume v0.3.x drops the sync pull connection when done.
                        # Verify the VM was created.
                        try:
                            check = await client.get(f"{lume_url}/lume/vms/{name}", timeout=10)
                            if check.status_code != 200 or check.json().get("status") in (
                                "",
                                None,
                                "error",
                            ):
                                raise RuntimeError(
                                    f"Lume pull failed for '{name}': connection dropped and VM not found"
                                    " (check GITHUB_TOKEN is set in lume's LaunchAgent plist)"
                                )
                        except httpx.HTTPError as e:
                            raise RuntimeError(f"Lume pull failed for '{name}': {e}") from e
                    print()
                    logger.info(f"Running Lume VM {name} (v0.3.x path)...")
                    run_resp = await client.post(
                        f"{lume_url}/lume/vms/{name}/run",
                        json={
                            "noDisplay": opts.get("no_display", True),
                            "sharedDirectories": opts.get("shared_directories", []),
                        },
                        timeout=120,
                    )
                    if run_resp.status_code >= 400:
                        try:
                            detail = run_resp.json()
                        except Exception:
                            detail = run_resp.text
                        raise RuntimeError(f"Lume failed to run VM '{name}': {detail}")
                    ip = await self._wait_for_ip(name, lume_url)
                    info = RuntimeInfo(host=ip, api_port=self.api_port, name=name)
                    await self.is_ready(info)
                    return info
                if start_resp.status_code == 404:
                    # Older lume without /pull/start — fall back to synchronous pull
                    logger.info(
                        "Lume does not support /pull/start, falling back to synchronous pull..."
                    )
                    try:
                        pull_resp = await client.post(
                            f"{lume_url}/lume/pull",
                            json=pull_payload,
                            timeout=1800,
                        )
                    except (httpx.ReadError, httpx.RemoteProtocolError):
                        # Lume v0.3.x drops the HTTP connection after pull
                        # completes. Check if the VM was actually created.
                        logger.info(
                            f"Lume sync pull connection dropped — checking if VM '{name}' was created..."
                        )
                        try:
                            check = await client.get(f"{lume_url}/lume/vms/{name}", timeout=10)
                            if check.status_code == 200:
                                vm_data = check.json()
                                if vm_data.get("status") not in ("", None, "error"):
                                    logger.info(
                                        f"VM '{name}' exists after connection drop — pull succeeded"
                                    )
                                    # fall through to run the VM
                                    pull_resp = check  # dummy, not used further
                            else:
                                raise RuntimeError(
                                    f"Lume pull failed for '{name}': connection dropped and VM not found"
                                    " (check GITHUB_TOKEN is set in lume's LaunchAgent plist)"
                                )
                        except httpx.HTTPError as check_err:
                            raise RuntimeError(
                                f"Lume pull failed for '{name}': connection dropped and could not verify VM: {check_err}"
                            ) from check_err
                    if pull_resp.status_code >= 400:
                        try:
                            detail = pull_resp.json()
                        except Exception:
                            detail = pull_resp.text
                        raise RuntimeError(f"Lume pull failed for '{name}': {detail}")
                elif start_resp.status_code >= 400:
                    try:
                        detail = start_resp.json()
                    except Exception:
                        detail = start_resp.text
                    raise RuntimeError(f"Lume pull/start failed for '{name}': {detail}")
                else:
                    # Poll /lume/vms/{name} until status leaves "pulling"
                    logger.info(f"Pulling image for '{name}'...")
                    pull_deadline = asyncio.get_event_loop().time() + 1800
                    last_progress = -1.0
                    while asyncio.get_event_loop().time() < pull_deadline:
                        try:
                            poll = await client.get(f"{lume_url}/lume/vms/{name}", timeout=10)
                        except (httpx.ReadError, httpx.RemoteProtocolError, httpx.ConnectError):
                            await asyncio.sleep(3)
                            continue
                        if poll.status_code == 200:
                            data = poll.json()
                            status = data.get("status", "")
                            progress = data.get("downloadProgress")
                            if progress is not None and progress != last_progress:
                                print(f"\rPulling macOS image: {progress:.0f}%", end="", flush=True)
                                last_progress = progress
                            if status == "pulling":
                                await asyncio.sleep(3)
                                continue
                            # Pull failed — lume surfaced an error via status
                            if "error" in status.lower():
                                raise RuntimeError(
                                    f"Lume pull failed for '{name}': {data.get('message', status)}"
                                )
                            break
                        elif poll.status_code >= 400:
                            # VM not found — pull may have failed before creating the VM
                            data = {}
                            try:
                                data = poll.json()
                            except Exception:
                                pass
                            raise RuntimeError(
                                f"Lume pull failed for '{name}': {data.get('message', poll.text)}"
                            )
                        await asyncio.sleep(3)
                    else:
                        raise TimeoutError(f"Lume pull for '{name}' did not complete within 1800s")
                    if last_progress >= 0:
                        print()  # newline after progress bar
                logger.info(f"Running Lume VM {name}...")
                run_resp = await client.post(
                    f"{lume_url}/lume/vms/{name}/run",
                    json={
                        "noDisplay": opts.get("no_display", True),
                        "sharedDirectories": opts.get("shared_directories", []),
                    },
                    timeout=120,
                )
                if run_resp.status_code >= 400:
                    try:
                        detail = run_resp.json()
                    except Exception:
                        detail = run_resp.text
                    raise RuntimeError(f"Lume failed to run VM '{name}': {detail}")

            ip = await self._wait_for_ip(name, lume_url)

        info = RuntimeInfo(host=ip, api_port=self.api_port, name=name)
        await self.is_ready(info)
        return info

    async def _wait_for_ip(self, name: str, lume_url: str, timeout: float = 300) -> str:
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=10) as client:
            while asyncio.get_event_loop().time() < deadline:
                resp = await client.get(f"{lume_url}/lume/vms/{name}")
                data = resp.json()
                status = data.get("status", "")
                if status == "stopped":
                    raise RuntimeError(f"Lume VM '{name}' is stopped — failed to start")
                ip = data.get("ip_address") or data.get("ipAddress")
                if ip and ip != "unknown" and not ip.startswith("0.0.0.0"):
                    return ip
                await asyncio.sleep(3)
        raise TimeoutError(f"Lume VM {name} did not get an IP within {timeout}s")

    async def suspend(self, name: str) -> None:
        """Stop (save state of) a Lume VM."""
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=30) as client:
            await client.post(f"{lume_url}/lume/vms/{name}/stop")

    async def resume(self, image: "Image", name: str, **opts) -> RuntimeInfo:
        """Start a stopped Lume VM and return its RuntimeInfo."""
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        async with httpx.AsyncClient(timeout=120) as client:
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

    async def list(self) -> list[dict]:
        """List all Lume VMs."""
        lume_url = f"http://{self.lume_host}:{self.lume_port}"
        try:
            async with httpx.AsyncClient(timeout=10) as client:
                resp = await client.get(f"{lume_url}/lume/vms")
                vms = resp.json()
                if isinstance(vms, dict):
                    vms = vms.get("vms", [])
        except Exception:
            return []
        result = []
        for vm in vms:
            raw = vm.get("status", "").lower()
            if raw == "running":
                status = "running"
            elif raw in ("stopped", "stop"):
                status = "suspended"
            else:
                status = raw
            result.append(
                {
                    "name": vm.get("name", ""),
                    "status": status,
                    "runtime_type": "lume",
                    "os_type": "macos",
                    "ip_address": vm.get("ip_address") or vm.get("ipAddress"),
                }
            )
        return result

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
