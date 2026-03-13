"""Cloud VM provider implementation using CUA Public API.

Implements the following public API endpoints:

- GET /v1/vms
- POST /v1/vms/:name/start
- POST /v1/vms/:name/stop
- POST /v1/vms/:name/restart
"""

import logging
from typing import Any, Dict, List, Optional

from ..base import BaseVMProvider, VMProviderType
from ..types import ListVMsResponse, MinimalVM

# Setup logging
logger = logging.getLogger(__name__)

import asyncio
import os
from urllib.parse import urlparse

import aiohttp

DEFAULT_API_BASE = os.getenv("CUA_API_BASE", "https://api.cua.ai")


class CloudProvider(BaseVMProvider):
    """Cloud VM Provider implementation."""

    def __init__(
        self,
        api_key: str,
        verbose: bool = False,
        api_base: Optional[str] = None,
        os_type: Optional[str] = None,
        **kwargs,
    ):
        """
        Args:
            api_key: API key for authentication
            verbose: Enable verbose logging
            api_base: Override for the CUA API base URL
            os_type: OS type of the VM ('macos', 'linux', etc.)
        """
        assert api_key, "api_key required for CloudProvider"
        self.api_key = api_key
        self.verbose = verbose
        self.api_base = (api_base or DEFAULT_API_BASE).rstrip("/")
        self.os_type = os_type

    @property
    def provider_type(self) -> VMProviderType:
        return VMProviderType.CLOUD

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        pass

    async def get_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Get VM information by querying the VM status endpoint.

        For macOS VMs: probes https://{name}-api.cua.sh/status (Cloudflare-proxied, port 443)
        For other VMs: probes https://{name}.containers.cloud.trycua.com:8443/status
        """
        hostname = await self.get_ip(name=name)

        # macOS VMs are Cloudflare-proxied (port 443), others use direct port 8443
        if self.os_type == "macos":
            base_url = f"https://{hostname}"
        else:
            base_url = f"https://{hostname}:8443"

        # Try HTTPS probe to the computer-server status endpoint
        try:
            timeout = aiohttp.ClientTimeout(total=3)
            async with aiohttp.ClientSession(timeout=timeout) as session:
                url = f"{base_url}/status"
                async with session.get(url, allow_redirects=False) as resp:
                    status_code = resp.status
                    vm_status: str
                    vm_os_type: Optional[str] = None
                    if status_code == 200:
                        try:
                            data = await resp.json(content_type=None)
                            vm_status = str(data.get("status", "ok"))
                            vm_os_type = str(data.get("os_type"))
                        except Exception:
                            vm_status = "unknown"
                    elif status_code < 500:
                        vm_status = "unknown"
                    else:
                        vm_status = "unknown"
                    return {
                        "name": name,
                        "status": "running" if vm_status == "ok" else vm_status,
                        "api_url": base_url,
                        "os_type": vm_os_type,
                    }
        except Exception:
            return {"name": name, "status": "not_found", "api_url": base_url}

    async def list_vms(self) -> ListVMsResponse:
        url = f"{self.api_base}/v1/vms"
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Accept": "application/json",
        }
        async with aiohttp.ClientSession() as session:
            async with session.get(url, headers=headers) as resp:
                if resp.status == 200:
                    try:
                        data = await resp.json(content_type=None)
                    except Exception:
                        text = await resp.text()
                        logger.error(f"Failed to parse list_vms JSON: {text}")
                        return []
                    if isinstance(data, list):
                        # Enrich with convenience URLs when possible.
                        enriched: List[Dict[str, Any]] = []
                        for item in data:
                            vm = dict(item) if isinstance(item, dict) else {}
                            name = vm.get("name")
                            password = vm.get("password")
                            source = vm.get("source", "")
                            is_macos = source == "macos-sandbox" or vm.get("os") == "macos"
                            if isinstance(name, str) and name:
                                if is_macos:
                                    host = f"{name}-api.cua.sh"
                                    if not vm.get("api_url"):
                                        vm["api_url"] = f"https://{host}"
                                else:
                                    host = f"{name}.containers.cloud.trycua.com"
                                    if not vm.get("api_url"):
                                        vm["api_url"] = f"https://{host}:8443"
                                    # vnc_url: only when password available
                                    if not vm.get("vnc_url") and isinstance(password, str) and password:
                                        vm["vnc_url"] = (
                                            f"https://{host}/vnc.html?autoconnect=true&password={password}"
                                        )
                            enriched.append(vm)
                        return enriched  # type: ignore[return-value]
                    logger.warning("Unexpected response for list_vms; expected list")
                    return []
                elif resp.status == 401:
                    logger.error("Unauthorized: invalid CUA API key for list_vms")
                    return []
                else:
                    text = await resp.text()
                    logger.error(f"list_vms failed: HTTP {resp.status} - {text}")
                    return []

    async def run_vm(
        self,
        name: str,
        image: Optional[str] = None,
        run_opts: Optional[Dict[str, Any]] = None,
        storage: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Start a VM via public API. Returns a minimal status."""
        url = f"{self.api_base}/v1/vms/{name}/start"
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Accept": "application/json",
        }
        async with aiohttp.ClientSession() as session:
            async with session.post(url, headers=headers) as resp:
                if resp.status in (200, 201, 202, 204):
                    return {"name": name, "status": "starting"}
                elif resp.status == 404:
                    return {"name": name, "status": "not_found"}
                elif resp.status == 401:
                    return {"name": name, "status": "unauthorized"}
                else:
                    text = await resp.text()
                    return {"name": name, "status": "error", "message": text}

    async def stop_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Stop a VM via public API."""
        url = f"{self.api_base}/v1/vms/{name}/stop"
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Accept": "application/json",
        }
        async with aiohttp.ClientSession() as session:
            async with session.post(url, headers=headers) as resp:
                if resp.status in (200, 202):
                    # Spec says 202 with {"status":"stopping"}
                    body_status: Optional[str] = None
                    try:
                        data = await resp.json(content_type=None)
                        body_status = data.get("status") if isinstance(data, dict) else None
                    except Exception:
                        body_status = None
                    return {"name": name, "status": body_status or "stopping"}
                elif resp.status == 404:
                    return {"name": name, "status": "not_found"}
                elif resp.status == 401:
                    return {"name": name, "status": "unauthorized"}
                else:
                    text = await resp.text()
                    return {"name": name, "status": "error", "message": text}

    async def restart_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Restart a VM via public API."""
        url = f"{self.api_base}/v1/vms/{name}/restart"
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Accept": "application/json",
        }
        async with aiohttp.ClientSession() as session:
            async with session.post(url, headers=headers) as resp:
                if resp.status in (200, 202):
                    # Spec says 202 with {"status":"restarting"}
                    body_status: Optional[str] = None
                    try:
                        data = await resp.json(content_type=None)
                        body_status = data.get("status") if isinstance(data, dict) else None
                    except Exception:
                        body_status = None
                    return {"name": name, "status": body_status or "restarting"}
                elif resp.status == 404:
                    return {"name": name, "status": "not_found"}
                elif resp.status == 401:
                    return {"name": name, "status": "unauthorized"}
                else:
                    text = await resp.text()
                    return {"name": name, "status": "error", "message": text}

    async def update_vm(
        self, name: str, update_opts: Dict[str, Any], storage: Optional[str] = None
    ) -> Dict[str, Any]:
        logger.warning("CloudProvider.update_vm is not implemented via public API")
        return {
            "name": name,
            "status": "unchanged",
            "message": "update_vm not supported by public API",
        }

    async def get_ip(
        self, name: Optional[str] = None, storage: Optional[str] = None, retry_delay: int = 2
    ) -> str:
        """
        Return the VM's hostname.

        For macOS VMs: '{name}-api.cua.sh' (Cloudflare-proxied)
        For other VMs: '{name}.containers.cloud.trycua.com'
        """
        if name is None:
            raise ValueError("VM name is required for CloudProvider.get_ip")
        if self.os_type == "macos":
            return f"{name}-api.cua.sh"
        return f"{name}.containers.cloud.trycua.com"
