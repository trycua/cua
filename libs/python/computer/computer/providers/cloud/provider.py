"""Cloud VM provider implementation using CUA Public API.

Implements the following public API endpoints:

- GET /v1/vms
- POST /v1/vms/:name/start
- POST /v1/vms/:name/stop
- POST /v1/vms/:name/restart
"""

import logging
from typing import Dict, List, Optional, Any

from ..base import BaseVMProvider, VMProviderType
from ..types import ListVMsResponse, MinimalVM

# Setup logging
logger = logging.getLogger(__name__)

import asyncio
import aiohttp
from urllib.parse import urlparse
import os


DEFAULT_API_BASE = os.getenv("CUA_API_BASE", "https://api.cua.ai")

class CloudProvider(BaseVMProvider):
    """Cloud VM Provider implementation."""
    def __init__(
        self,
        api_key: str,
        verbose: bool = False,
        api_base: Optional[str] = None,
        **kwargs,
    ):
        """
        Args:
            api_key: API key for authentication
            name: Name of the VM
            verbose: Enable verbose logging
        """
        assert api_key, "api_key required for CloudProvider"
        self.api_key = api_key
        self.verbose = verbose
        self.api_base = (api_base or DEFAULT_API_BASE).rstrip("/")

    @property
    def provider_type(self) -> VMProviderType:
        return VMProviderType.CLOUD

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        pass

    async def get_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Get VM information by querying the VM status endpoint.

        - Build hostname via get_ip(name) → "{name}.containers.cloud.trycua.com"
        - Probe https://{hostname}:8443/status with a short timeout
        - If JSON contains a "status" field, return it; otherwise infer
        - Fallback to DNS resolve check to distinguish unknown vs not_found
        """
        hostname = await self.get_ip(name=name)

        # Try HTTPS probe to the computer-server status endpoint (8443)
        try:
            timeout = aiohttp.ClientTimeout(total=3)
            async with aiohttp.ClientSession(timeout=timeout) as session:
                url = f"https://{hostname}:8443/status"
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
                        "api_url": f"https://{hostname}:8443",
                        "os_type": vm_os_type,
                    }
        except Exception:
            return {"name": name, "status": "not_found", "api_url": f"https://{hostname}:8443"}

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
                            if isinstance(name, str) and name:
                                host = f"{name}.containers.cloud.trycua.com"
                                # api_url: always set if missing
                                if not vm.get("api_url"):
                                    vm["api_url"] = f"https://{host}:8443"
                                # vnc_url: only when password available
                                if not vm.get("vnc_url") and isinstance(password, str) and password:
                                    vm[
                                        "vnc_url"
                                    ] = f"https://{host}/vnc.html?autoconnect=true&password={password}"
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

    async def run_vm(self, name: str, image: Optional[str] = None, run_opts: Optional[Dict[str, Any]] = None, storage: Optional[str] = None) -> Dict[str, Any]:
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

    async def update_vm(self, name: str, update_opts: Dict[str, Any], storage: Optional[str] = None) -> Dict[str, Any]:
        logger.warning("CloudProvider.update_vm is not implemented via public API")
        return {"name": name, "status": "unchanged", "message": "update_vm not supported by public API"}

    async def get_ip(self, name: Optional[str] = None, storage: Optional[str] = None, retry_delay: int = 2) -> str:
        """
        Return the VM's IP address as '{container_name}.containers.cloud.trycua.com'.
        Uses the provided 'name' argument (the VM name requested by the caller),
        falling back to self.name only if 'name' is None.
        Retries up to 3 times with retry_delay seconds if hostname is not available.
        """
        if name is None:
            raise ValueError("VM name is required for CloudProvider.get_ip")
        return f"{name}.containers.cloud.trycua.com"
