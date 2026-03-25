"""CloudTransport — connects to a CUA cloud VM via the platform API.

Resolves VM connection info from the API, optionally creates a new VM,
then delegates all computer control to an inner HTTPTransport pointed at
the VM's computer-server endpoint.
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any, Dict, Optional

import httpx
from cua_sandbox._config import get_api_key, get_base_url
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.http import HTTPTransport

logger = logging.getLogger(__name__)

_POLL_INTERVAL = 2.0  # seconds between status polls
_POLL_TIMEOUT = 600.0  # max seconds to wait for VM to be running


class CloudTransport(Transport):
    """Transport that provisions / connects to a CUA cloud VM."""

    _DEFAULT_CPU = 1
    _DEFAULT_MEMORY_MB = 4096
    _DEFAULT_DISK_GB = 64

    def __init__(
        self,
        name: Optional[str] = None,
        *,
        api_key: Optional[str] = None,
        base_url: Optional[str] = None,
        # Creation params (used only when creating a new VM)
        image: Optional[Any] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
    ):
        self._name = name
        self._api_key_override = api_key
        self._base_url = base_url or get_base_url()
        self._image = image
        self._cpu = cpu
        self._memory_mb = memory_mb
        self._disk_gb = disk_gb
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
            logger.debug("[cloud] getting VM info for %r", self._name)
            vm_info = await self._get_vm(self._name)
            logger.debug("[cloud] VM info: status=%r", vm_info.get("status"))
        else:
            logger.debug("[cloud] creating new VM")
            vm_info = await self._create_vm()
            self._name = vm_info["name"]
            logger.debug("[cloud] created VM %r", self._name)

        # Poll until running
        logger.debug(
            "[cloud] waiting for VM %r to be running (status=%r)", self._name, vm_info.get("status")
        )
        vm_info = await self._wait_for_running(vm_info)
        logger.debug("[cloud] VM %r is running", self._name)

        # Resolve computer-server endpoint — auth with CUA API key + container name
        cs_url = self._resolve_endpoint(vm_info)
        logger.debug("[cloud] resolved endpoint: %s", cs_url)
        self._inner = HTTPTransport(cs_url, api_key=api_key, container_name=self._name)
        logger.debug("[cloud] connecting inner HTTPTransport")
        await self._inner.connect()
        logger.debug("[cloud] HTTPTransport connected")

        # Wait for computer-server to be reachable (it may lag behind VM "running" status)
        logger.debug("[cloud] waiting for computer-server to be ready")
        await self._wait_for_server_ready()
        logger.debug("[cloud] computer-server ready")

        # Apply image layers (e.g. APK installs) after server is ready
        if self._image and self._image._layers:
            logger.debug("[cloud] applying image layers")
            await self._apply_image_layers()

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

    async def suspend_vm(self) -> None:
        """Stop (suspend) the cloud VM."""
        if not self._name:
            return
        assert self._api_client
        await self._api_client.post(f"/v1/vms/{self._name}/stop")

    async def resume_vm(self) -> None:
        """Start (resume) the cloud VM."""
        if not self._name:
            return
        assert self._api_client
        await self._api_client.post(f"/v1/vms/{self._name}/run")

    async def restart_vm(self) -> None:
        """Restart the cloud VM."""
        if not self._name:
            return
        assert self._api_client
        await self._api_client.post(f"/v1/vms/{self._name}/restart")

    # ── Delegated methods ───────────────────────────────────────────────

    async def send(self, action: str, **params: Any) -> Any:
        assert self._inner, "Transport not connected"
        return await self._inner.send(action, **params)

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        assert self._inner, "Transport not connected"
        return await self._inner.screenshot(format=format, quality=quality)

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._inner, "Transport not connected"
        return await self._inner.get_screen_size()

    async def get_environment(self) -> str:
        assert self._inner, "Transport not connected"
        return await self._inner.get_environment()

    async def get_display_url(self, *, share: bool = False) -> str:
        if not self._name:
            raise ValueError("Transport not connected — no VM name available")
        if not share:
            return f"https://cua.ai/connect/incus/{self._name}"
        vm_info = await self._get_vm(self._name)
        password = vm_info.get("password", "")
        for ep in vm_info.get("endpoints", []):
            if ep.get("name") == "vnc":
                host = ep["host"]
                url = f"https://{host}"
                if password:
                    url += f"/?password={password}"
                return url
        raise ValueError(
            f"VM '{self._name}' has no VNC endpoint. "
            "Only Android and desktop VMs expose a VNC endpoint."
        )

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
                "  Sandbox.create(image=Image.linux())  or  Sandbox.create(image=Image.windows())  or  Sandbox.create(image=Image.macos())\n"
                "Or connect to an existing VM by name: Sandbox.connect(name='my-vm')"
            )
        os_type = getattr(self._image, "os_type", None)
        if not os_type:
            raise ValueError(
                "Image must have an os_type. Use Image.linux(), Image.windows(), or Image.macos()."
            )
        body: Dict[str, Any] = {
            "os": os_type,
            "region": self._region,
        }
        # If any resource spec is provided, send explicit specs (defaulting missing to small).
        # Otherwise, send configuration="small" for backwards compat with legacy API.
        if any(v is not None for v in (self._cpu, self._memory_mb, self._disk_gb)):
            body["cpu"] = self._cpu or self._DEFAULT_CPU
            body["memoryMb"] = self._memory_mb or self._DEFAULT_MEMORY_MB
            body["diskGb"] = self._disk_gb or self._DEFAULT_DISK_GB
        else:
            body["configuration"] = "small"
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
            logger.debug(
                "[cloud] _wait_for_running: elapsed=%.0fs status=%r", elapsed, vm_info.get("status")
            )
        return vm_info

    async def _wait_for_server_ready(self) -> None:
        """Poll the computer-server until it responds (retries on connection/HTTP errors)."""
        assert self._inner
        elapsed = 0.0
        last_err: Optional[Exception] = None
        while elapsed < _POLL_TIMEOUT:
            try:
                await self._inner.get_screen_size()
                return  # Server is ready
            except httpx.HTTPStatusError as e:
                if e.response.status_code < 500:
                    raise  # 4xx errors are not transient — fail fast
                last_err = e
                logger.debug("[cloud] _wait_for_server_ready: elapsed=%.0fs err=%r", elapsed, e)
                await asyncio.sleep(_POLL_INTERVAL)
                elapsed += _POLL_INTERVAL
            except Exception as e:
                last_err = e
                logger.debug("[cloud] _wait_for_server_ready: elapsed=%.0fs err=%r", elapsed, e)
                await asyncio.sleep(_POLL_INTERVAL)
                elapsed += _POLL_INTERVAL
        raise TimeoutError(
            f"Computer-server for VM {self._name!r} not reachable within {_POLL_TIMEOUT}s: {last_err}"
        )

    async def _apply_image_layers(self) -> None:
        """Apply image layers (APK installs, shell commands) after the VM is ready."""
        import base64
        import hashlib
        import urllib.request
        from pathlib import Path

        assert self._inner
        for layer in self._image._layers:
            lt = layer["type"]
            if lt == "apk_install":
                for apk in layer["packages"]:
                    dest = "/data/local/tmp/cua_install.apk"
                    # Resolve APK to local bytes (download URL if needed)
                    if apk.startswith(("http://", "https://")):
                        cache_dir = Path.home() / ".cua" / "cua-sandbox" / "apk-cache"
                        cache_dir.mkdir(parents=True, exist_ok=True)
                        cache_file = cache_dir / (
                            hashlib.sha256(apk.encode()).hexdigest()[:16] + ".apk"
                        )
                        if not cache_file.exists():
                            urllib.request.urlretrieve(apk, cache_file)
                        apk_bytes = cache_file.read_bytes()
                    else:
                        apk_bytes = Path(apk).read_bytes()
                    # Push to device via write_bytes (base64), then install
                    await self._inner.send(
                        "write_bytes",
                        path=dest,
                        content_b64=base64.b64encode(apk_bytes).decode(),
                    )
                    # Install; on signature mismatch uninstall existing and retry.
                    # The script always exits 0 so run_command does not raise.
                    await self._inner.send(
                        "run_command",
                        command=(
                            f"out=$(pm install -r {dest} 2>&1); "
                            f'if echo "$out" | grep -q INSTALL_FAILED_UPDATE_INCOMPATIBLE; then '
                            f'  pkg=$(echo "$out" | sed -n "s/.*Package \\(\\S*\\) signatures.*/\\1/p"); '
                            f'  pm uninstall "$pkg"; pm install -r {dest}; '
                            f'else echo "$out"; fi; true'
                        ),
                        timeout=90,
                    )
            elif lt == "run":
                await self._inner.send("run_command", command=layer["command"], timeout=60)

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


async def cloud_list_vms(
    *, api_key: Optional[str] = None, base_url: Optional[str] = None
) -> list[dict]:
    """List all cloud VMs. Returns raw VM dicts from the API."""
    from cua_sandbox._config import get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError("No CUA API key. Set CUA_API_KEY or run cua.login().")
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
    from cua_sandbox._config import get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError("No CUA API key. Set CUA_API_KEY or run cua.login().")
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
    from cua_sandbox._config import get_api_key, get_base_url

    key = get_api_key(api_key)
    if not key:
        raise ValueError("No CUA API key. Set CUA_API_KEY or run cua.login().")
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
