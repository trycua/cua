"""CloudTransport — connects to a CUA cloud VM via the platform API.

Resolves VM connection info from the API, optionally creates a new VM,
then delegates all computer control to an inner HTTPTransport pointed at
the VM's computer-server endpoint.
"""

from __future__ import annotations

import asyncio
from typing import Any, Dict, Optional

import httpx
from cua_sandbox._config import get_api_key, get_base_url
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.http import HTTPTransport

_POLL_INTERVAL = 2.0  # seconds between status polls
_POLL_TIMEOUT = 300.0  # max seconds to wait for VM to be running


class CloudTransport(Transport):
    """Transport that provisions / connects to a CUA cloud VM."""

    def __init__(
        self,
        name: Optional[str] = None,
        *,
        api_key: Optional[str] = None,
        base_url: Optional[str] = None,
        # Creation params (used only when creating a new VM)
        image: Optional[Any] = None,
        configuration: str = "small",
        region: str = "us-east-1",
    ):
        self._name = name
        self._api_key_override = api_key
        self._base_url = base_url or get_base_url()
        self._image = image
        self._configuration = configuration
        self._region = region
        self._inner: Optional[HTTPTransport] = None
        self._api_client: Optional[httpx.AsyncClient] = None

    # ── Connection lifecycle ────────────────────────────────────────────

    async def connect(self) -> None:
        api_key = get_api_key(self._api_key_override)
        if not api_key:
            raise ValueError(
                "No CUA API key found. Cloud sandboxes are the default — to use one, provide an API key via:\n"
                "  1. cua.configure(api_key='sk-...')\n"
                "  2. Set the CUA_API_KEY environment variable\n"
                "  3. Run cua.login() to authenticate via browser\n"
                "  4. Pass api_key='sk-...' directly to sandbox()\n"
                "\n"
                "For local-only usage (no cloud), use sandbox(local=True) instead."
            )

        self._api_client = httpx.AsyncClient(
            base_url=self._base_url,
            headers={"Authorization": f"Bearer {api_key}"},
            timeout=30.0,
        )

        if self._name:
            vm_info = await self._get_vm(self._name)
        else:
            vm_info = await self._create_vm()
            self._name = vm_info["name"]

        # Poll until running
        vm_info = await self._wait_for_running(vm_info)

        # Resolve computer-server endpoint — auth with CUA API key + container name
        cs_url = self._resolve_endpoint(vm_info)
        self._inner = HTTPTransport(cs_url, api_key=api_key, container_name=self._name)
        await self._inner.connect()

    async def disconnect(self) -> None:
        if self._inner:
            await self._inner.disconnect()
            self._inner = None
        if self._api_client:
            await self._api_client.aclose()
            self._api_client = None

    async def delete_vm(self) -> None:
        """Delete the cloud VM via the platform API."""
        api_key = get_api_key(self._api_key_override)
        if not api_key or not self._name:
            return
        async with httpx.AsyncClient(
            base_url=self._base_url,
            headers={"Authorization": f"Bearer {api_key}"},
            timeout=30.0,
        ) as client:
            await client.delete(f"/v1/vms/{self._name}")

    # ── Delegated methods ───────────────────────────────────────────────

    async def send(self, action: str, **params: Any) -> Any:
        assert self._inner, "Transport not connected"
        return await self._inner.send(action, **params)

    async def screenshot(self) -> bytes:
        assert self._inner, "Transport not connected"
        return await self._inner.screenshot()

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._inner, "Transport not connected"
        return await self._inner.get_screen_size()

    async def get_environment(self) -> str:
        assert self._inner, "Transport not connected"
        return await self._inner.get_environment()

    # ── Helpers ─────────────────────────────────────────────────────────

    @property
    def name(self) -> Optional[str]:
        return self._name

    async def _get_vm(self, name: str) -> dict:
        assert self._api_client
        resp = await self._api_client.get(f"/v1/vms/{name}")
        resp.raise_for_status()
        return resp.json()

    async def _create_vm(self) -> dict:
        assert self._api_client
        if not self._image:
            raise ValueError(
                "Cannot create a cloud VM without an image. Use:\n"
                "  sandbox(image=Image.linux())  or  sandbox(image=Image.windows())  or  sandbox(image=Image.macos())\n"
                "Or connect to an existing VM by name: sandbox(name='my-vm')"
            )
        os_type = getattr(self._image, "os_type", None)
        if not os_type:
            raise ValueError(
                "Image must have an os_type. Use Image.linux(), Image.windows(), or Image.macos()."
            )
        body: Dict[str, Any] = {
            "os": os_type,
            "configuration": self._configuration,
            "region": self._region,
        }
        resp = await self._api_client.post("/v1/vms", json=body)
        resp.raise_for_status()
        return resp.json()

    async def _wait_for_running(self, vm_info: dict) -> dict:
        """Poll until the VM status is 'running' (or 'ready')."""
        elapsed = 0.0
        while vm_info.get("status") not in ("running", "ready"):
            if elapsed >= _POLL_TIMEOUT:
                raise TimeoutError(
                    f"VM {self._name!r} did not become running within {_POLL_TIMEOUT}s "
                    f"(last status: {vm_info.get('status')})"
                )
            await asyncio.sleep(_POLL_INTERVAL)
            elapsed += _POLL_INTERVAL
            vm_info = await self._get_vm(self._name)  # type: ignore[arg-type]
        return vm_info

    @staticmethod
    def _resolve_endpoint(vm_info: dict) -> str:
        """Build the computer-server HTTP URL from VM info."""
        # Prefer explicit endpoints array (Incus VMs)
        for ep in vm_info.get("endpoints", []):
            if ep.get("name") in ("computer-server", "api"):
                host = ep["host"]
                # cua.sh hosts are behind a reverse proxy — don't append port
                if host.endswith(".cua.sh"):
                    return f"https://{host}"
                return f"http://{host}:{ep['port']}"

        # Fallback: legacy host-based URL
        host = vm_info.get("host")
        if not host:
            raise ValueError(f"Cannot resolve computer-server endpoint from VM info: {vm_info}")
        return f"http://{host}:8000"
