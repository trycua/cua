"""Cloud VM provider implementation using Cua Public API.

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
from core.http import cua_version_headers

DEFAULT_API_BASE = os.getenv("CUA_API_BASE", "https://api.cua.ai")


class CloudProvider(BaseVMProvider):
    """Cloud VM Provider implementation."""

    def __init__(
        self,
        api_key: Optional[str] = None,
        verbose: bool = False,
        api_base: Optional[str] = None,
        **kwargs,
    ):
        """
        Args:
            api_key: API key for authentication (defaults to CUA_API_KEY environment variable)
            name: Name of the VM
            verbose: Enable verbose logging
        """
        # Fall back to environment variable if api_key not provided
        if api_key is None:
            api_key = os.getenv("CUA_API_KEY")
        assert (
            api_key
        ), "api_key required for CloudProvider (provide via parameter or CUA_API_KEY environment variable)"
        self.api_key = api_key
        self.verbose = verbose
        self.api_base = (api_base or DEFAULT_API_BASE).rstrip("/")
        # Host caching dictionary: {vm_name: host_string}
        self._host_cache: Dict[str, str] = {}

    def _base_headers(self) -> Dict[str, str]:
        return {
            "Authorization": f"Bearer {self.api_key}",
            "Accept": "application/json",
            **cua_version_headers(),
        }

    @property
    def provider_type(self) -> VMProviderType:
        return VMProviderType.CLOUD

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        pass

    async def get_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Get VM information via the public API and optionally probe for os_type.

        Uses GET /v1/vms/:name as source of truth for VM existence and status,
        then probes the computer-server for supplementary info (os_type).
        """
        hostname = await self._get_host_for_vm(name)
        api_url = f"https://{hostname}:8443"

        # Query the API for authoritative VM info
        url = f"{self.api_base}/v1/vms/{name}"
        headers = self._base_headers()
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(url, headers=headers) as resp:
                    if resp.status == 404:
                        return {"name": name, "status": "not_found", "api_url": api_url}
                    if resp.status == 401:
                        return {"name": name, "status": "unauthorized", "api_url": api_url}
                    if resp.status != 200:
                        text = await resp.text()
                        logger.error(f"get_vm API error: HTTP {resp.status} - {text}")
                        return {"name": name, "status": "unknown", "api_url": api_url}
                    vm_info = await resp.json(content_type=None)
        except Exception as e:
            logger.error(f"get_vm API request failed: {e}")
            return {"name": name, "status": "unknown", "api_url": api_url}

        # Enrich with host-derived URLs
        host = vm_info.get("host")
        if isinstance(host, str) and host:
            self._host_cache[name] = host
            hostname = host
            api_url = f"https://{hostname}:8443"
        vm_info["api_url"] = api_url

        password = vm_info.get("password")
        if not vm_info.get("vnc_url") and isinstance(password, str) and password:
            vm_info["vnc_url"] = f"https://{hostname}/vnc.html?autoconnect=true&password={password}"

        # Map "os" from API to "os_type" for backward compatibility
        if vm_info.get("os") and not vm_info.get("os_type"):
            vm_info["os_type"] = vm_info["os"]

        return vm_info

    async def list_vms(self) -> ListVMsResponse:
        url = f"{self.api_base}/v1/vms"
        headers = self._base_headers()
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
                            api_host = vm.get("host")  # Read host from API response

                            if isinstance(name, str) and name:
                                # Use host from API if available, otherwise fallback to legacy format
                                if isinstance(api_host, str) and api_host:
                                    host = api_host
                                    # Cache the host for this VM
                                    self._host_cache[name] = host
                                else:
                                    # Legacy fallback
                                    host = f"{name}.containers.cloud.trycua.com"
                                    # Cache the legacy host
                                    self._host_cache[name] = host

                                # api_url: always set if missing
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
                    logger.error("Unauthorized: invalid Cua API key for list_vms")
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
        headers = self._base_headers()
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
        headers = self._base_headers()
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
        headers = self._base_headers()
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

    async def _get_host_for_vm(self, name: str) -> str:
        """
        Get the host for a VM, trying multiple approaches:
        1. Check cache first
        2. Try to refresh cache by calling list_vms
        3. Try .sandbox.cua.ai format
        4. Fallback to legacy .containers.cloud.trycua.com format

        Args:
            name: VM name

        Returns:
            Host string for the VM
        """
        # Check cache first
        if name in self._host_cache:
            return self._host_cache[name]

        # Try to refresh cache by calling list_vms
        try:
            await self.list_vms()
            # Check cache again after refresh
            if name in self._host_cache:
                return self._host_cache[name]
        except Exception as e:
            logger.warning(f"Failed to refresh VM list for host lookup: {e}")

        # Try .sandbox.cua.ai format first
        sandbox_host = f"{name}.sandbox.cua.ai"
        if await self._test_host_connectivity(sandbox_host):
            self._host_cache[name] = sandbox_host
            return sandbox_host

        # Fallback to legacy format
        legacy_host = f"{name}.containers.cloud.trycua.com"
        # Cache the legacy host
        self._host_cache[name] = legacy_host
        return legacy_host

    async def _test_host_connectivity(self, hostname: str) -> bool:
        """
        Test if a host is reachable by trying to connect to its status endpoint.

        Args:
            hostname: Host to test

        Returns:
            True if host is reachable, False otherwise
        """
        try:
            timeout = aiohttp.ClientTimeout(total=2)  # Short timeout for connectivity test
            async with aiohttp.ClientSession(timeout=timeout) as session:
                url = f"https://{hostname}:8443/status"
                async with session.get(url, allow_redirects=False) as resp:
                    # Any response (even error) means the host is reachable
                    return True
        except Exception:
            return False

    async def get_ip(
        self, name: Optional[str] = None, storage: Optional[str] = None, retry_delay: int = 2
    ) -> str:
        """
        Return the VM's host address, trying to use cached host from API or falling back to legacy format.
        Uses the provided 'name' argument (the VM name requested by the caller).
        """
        if name is None:
            raise ValueError("VM name is required for CloudProvider.get_ip")

        return await self._get_host_for_vm(name)
