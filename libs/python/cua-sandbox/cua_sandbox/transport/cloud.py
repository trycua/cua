"""Legacy API-key cloud (api.cua.ai) compatibility.

API-key cloud VMs were driven through computer-server, which cua-spacesd
replaced; computer-server and its transports were removed. Cloud sandboxes
now run on Fleet (OAuth client credentials or a Fleet workload token):

    import cua_sandbox as cua
    cua.configure(client_id="...", client_secret="...")   # or CUA_CLIENT_ID/SECRET
    pool = await cua.Pool.apply(cua.Image.linux(), name="my-pool")
    async with pool.claim() as sb: ...

:class:`CloudTransport` stays importable (it is a public export) and fails
with that guidance. The REST helpers below still list, inspect and delete
existing API-key VMs so they can be cleaned up.
"""

from __future__ import annotations

import logging
from typing import Any, Dict, Optional

import httpx
from cua_sandbox._config import FLEET_CREDENTIALS_MISSING, get_api_key, get_base_url
from cua_sandbox.transport.base import Transport

logger = logging.getLogger(__name__)

# No legacy api.cua.ai key and no Fleet credentials.
_NO_KEY_MESSAGE = FLEET_CREDENTIALS_MISSING

LEGACY_CLOUD_REMOVED = (
    "API-key cloud VMs (api.cua.ai) were driven through computer-server, which has "
    "been removed in favour of cua-spacesd. Use Fleet cloud sandboxes instead: "
    "configure OAuth client credentials (cua.configure(client_id=..., client_secret=...) "
    "or CUA_CLIENT_ID/CUA_CLIENT_SECRET, or FLEETS_TOKEN), then "
    "`pool = await Pool.apply(image, name=...)` and `Sandbox.create(pool=pool, ...)` "
    "or `Sandbox.ephemeral(image)`. Existing API-key VMs can still be listed and "
    "deleted with Sandbox.list(api_key=...) / Sandbox.delete(name, api_key=...)."
)


class LegacyCloudRemovedError(RuntimeError):
    """The API-key cloud data plane (computer-server) no longer exists."""


class CloudTransport(Transport):
    """Compatibility shim for the removed API-key cloud transport.

    Constructing it still works (and keeps the requested image), so code that
    builds one fails at ``connect()`` with a message pointing to Fleet.
    """

    def __init__(
        self,
        name: Optional[str] = None,
        *,
        api_key: Optional[str] = None,
        base_url: Optional[str] = None,
        image: Optional[Any] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
    ):
        self._name = name
        self._api_key_override = api_key
        self._base_url = base_url or get_base_url()
        self._image = image
        self._cpu = cpu
        self._memory_mb = memory_mb
        self._disk_gb = disk_gb
        self._region = region

    @property
    def name(self) -> Optional[str]:
        return self._name

    async def connect(self) -> None:
        if not get_api_key(self._api_key_override):
            raise ValueError(_NO_KEY_MESSAGE)
        if not self._name and self._image is None:
            raise ValueError("Cannot create a cloud VM without an image")
        raise LegacyCloudRemovedError(LEGACY_CLOUD_REMOVED)

    async def disconnect(self) -> None:
        return None

    async def delete_vm(self) -> None:
        """Delete an existing API-key VM through the platform API."""
        if not self._name or not get_api_key(self._api_key_override):
            return
        await cloud_vm_action(self._name, "delete", api_key=self._api_key_override)

    async def create_snapshot(self, name: str | None = None, stateful: bool = False) -> dict:
        raise LegacyCloudRemovedError(LEGACY_CLOUD_REMOVED)

    async def send(self, action: str, **params: Any) -> Any:
        raise LegacyCloudRemovedError(LEGACY_CLOUD_REMOVED)

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        raise LegacyCloudRemovedError(LEGACY_CLOUD_REMOVED)

    async def get_screen_size(self) -> Dict[str, int]:
        raise LegacyCloudRemovedError(LEGACY_CLOUD_REMOVED)

    async def get_environment(self) -> str:
        raise LegacyCloudRemovedError(LEGACY_CLOUD_REMOVED)

    @staticmethod
    async def _build_pwa2apk(*args: Any, **kwargs: Any) -> tuple:
        """Moved to :func:`cua_sandbox.builder.pwa2apk.build_pwa2apk`."""
        from cua_sandbox.builder.pwa2apk import build_pwa2apk

        return await build_pwa2apk(*args, **kwargs)


async def cloud_list_vms(
    *, api_key: Optional[str] = None, base_url: Optional[str] = None
) -> list[dict]:
    """List all cloud VMs. Returns raw VM dicts from the API."""
    from cua_sandbox._config import FLEET_CREDENTIALS_MISSING, get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError(FLEET_CREDENTIALS_MISSING)
    url = base_url or get_base_url()
    async with httpx.AsyncClient(
        base_url=url,
        headers={"Authorization": f"Bearer {key}"},
        timeout=30.0,
    ) as client:
        resp = await client.get("/v1/vms")
        resp.raise_for_status()
        data = resp.json()
        return data if isinstance(data, list) else data.get("vms", [])


async def cloud_get_vm(
    name: str, *, api_key: Optional[str] = None, base_url: Optional[str] = None
) -> dict:
    """Get info for a single cloud VM by name."""
    from cua_sandbox._config import FLEET_CREDENTIALS_MISSING, get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError(FLEET_CREDENTIALS_MISSING)
    url = base_url or get_base_url()
    async with httpx.AsyncClient(
        base_url=url,
        headers={"Authorization": f"Bearer {key}"},
        timeout=30.0,
    ) as client:
        resp = await client.get(f"/v1/vms/{name}")
        resp.raise_for_status()
        return resp.json()


async def cloud_vm_action(
    name: str,
    action: str,
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> None:
    """POST /v1/vms/{name}/{action}. action is 'stop', 'run', 'restart', or 'delete'."""
    from cua_sandbox._config import FLEET_CREDENTIALS_MISSING, get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError(FLEET_CREDENTIALS_MISSING)
    url = base_url or get_base_url()
    async with httpx.AsyncClient(
        base_url=url,
        headers={"Authorization": f"Bearer {key}"},
        timeout=30.0,
    ) as client:
        if action == "delete":
            await client.delete(f"/v1/vms/{name}")
        else:
            await client.post(f"/v1/vms/{name}/{action}")
