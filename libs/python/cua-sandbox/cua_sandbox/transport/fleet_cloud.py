"""Fleet-backed implementation of the public cloud sandbox transport."""

from __future__ import annotations

import asyncio
import logging
import threading
import time
from typing import TYPE_CHECKING, Any, Optional
from urllib.parse import urlparse

import httpx
from cua_sandbox._config import (
    get_client_id,
    get_client_secret,
    get_fleet_base_url,
    get_token_url,
)
from cua_sandbox.image import Image
from cua_sandbox.transport.fleet import FleetTransport

if TYPE_CHECKING:
    from cua_sandbox.interfaces.tunnel import TunnelInfo

logger = logging.getLogger(__name__)


class _FleetClient:
    """Minimal synchronous Fleet client used by the async transport wrapper."""

    def __init__(self) -> None:
        client_id = get_client_id()
        client_secret = get_client_secret()
        if not client_id or not client_secret:
            raise ValueError(
                "Fleet cloud sandboxes require CUA_CLIENT_ID and CUA_CLIENT_SECRET, "
                "or cua.configure(client_id=..., client_secret=...)."
            )
        token_response = httpx.post(
            get_token_url(),
            data={
                "grant_type": "client_credentials",
                "client_id": client_id,
                "client_secret": client_secret,
            },
            headers={"Accept": "application/json"},
            timeout=30.0,
        )
        token_response.raise_for_status()
        self._base_url = get_fleet_base_url().rstrip("/")
        self._headers = {"Authorization": f"Bearer {token_response.json()['access_token']}"}

    def create_pool(self, request: dict[str, Any]) -> dict[str, Any]:
        namespace = request["namespace"]
        namespace_response = httpx.post(
            f"{self._base_url}/api/namespaces",
            json={"name": namespace},
            headers=self._headers,
            timeout=30.0,
        )
        if namespace_response.status_code not in {200, 201, 202, 409}:
            namespace_response.raise_for_status()
        namespace_created = namespace_response.status_code != 409
        body = {
            "apiVersion": "cua.ai/v1",
            "kind": "OSGymWorkspacePool",
            "metadata": {"name": namespace, "namespace": namespace},
            "spec": request["spec"],
        }
        response = httpx.post(
            self._pool_url(namespace), json=body, headers=self._headers, timeout=30.0
        )
        try:
            response.raise_for_status()
        except BaseException:
            if namespace_created:
                self.delete_namespace(namespace)
            raise
        return response.json()

    def create_claim(self, request: dict[str, Any]) -> dict[str, Any]:
        time_to_start = request.get("time_to_start")
        pool = (
            self.wait_pool_ready(request["pool"], time_to_start)
            if time_to_start is not None
            else self.wait_pool_ready(request["pool"])
        )
        namespace = pool["metadata"]["namespace"]
        name = f"{pool['metadata']['name']}-claim"
        body = {
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxClaim",
            "metadata": {"name": name, "namespace": namespace},
            "spec": {"sandboxTemplateRef": {"name": f"{pool['metadata']['name']}-template"}},
        }
        response = httpx.post(
            self._claim_url(namespace), json=body, headers=self._headers, timeout=30.0
        )
        response.raise_for_status()
        return response.json()

    def wait_claim(
        self,
        claim: dict[str, Any],
        *,
        services: Optional[list[str]] = None,
        time_to_start: Optional[float] = None,
    ) -> dict[str, Any]:
        namespace = claim["metadata"]["namespace"]
        name = claim["metadata"]["name"]
        timeout = time_to_start if time_to_start is not None else 600.0
        deadline = time.monotonic() + timeout
        while True:
            response = httpx.get(
                f"{self._claim_url(namespace)}/{name}", headers=self._headers, timeout=30.0
            )
            response.raise_for_status()
            current = response.json()
            status = current.get("status") or {}
            phase = status.get("phase", "Pending")
            sandbox = (status.get("sandbox") or {}).get("name")
            if phase == "Bound" and sandbox:
                return {
                    "namespace": namespace,
                    "sandbox": sandbox,
                    "services": services or ["server"],
                }
            if phase in {"Failed", "Error", "Expired"}:
                raise RuntimeError(f"Fleet claim {name!r} failed: {status}")
            if time.monotonic() >= deadline:
                raise TimeoutError(f"Fleet claim {name!r} did not bind within {timeout} seconds")
            time.sleep(1)

    def delete_claim(self, claim: dict[str, Any]) -> None:
        metadata = claim["metadata"]
        httpx.delete(
            f"{self._claim_url(metadata['namespace'])}/{metadata['name']}",
            headers=self._headers,
            timeout=30.0,
        ).raise_for_status()

    def delete_pool(self, pool: dict[str, Any]) -> None:
        metadata = pool["metadata"]
        response = httpx.delete(
            f"{self._pool_url(metadata['namespace'])}/{metadata['name']}",
            headers=self._headers,
            timeout=30.0,
        )
        if response.status_code not in {200, 202, 204, 404}:
            response.raise_for_status()
        self.delete_namespace(metadata["namespace"])

    def delete_namespace(self, namespace: str) -> None:
        response = httpx.delete(
            f"{self._base_url}/api/namespaces/{namespace}",
            headers=self._headers,
            timeout=30.0,
        )
        if response.status_code not in {200, 202, 204, 404}:
            response.raise_for_status()

    def wait_pool_ready(
        self, pool: dict[str, Any], time_to_start: Optional[float] = None
    ) -> dict[str, Any]:
        metadata = pool["metadata"]
        timeout = time_to_start if time_to_start is not None else 600.0
        deadline = time.monotonic() + timeout
        while True:
            response = httpx.get(
                f"{self._pool_url(metadata['namespace'])}/{metadata['name']}",
                headers=self._headers,
                timeout=30.0,
            )
            response.raise_for_status()
            current = response.json()
            if (current.get("status") or {}).get("availableCount", 0) > 0:
                return current
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"Fleet pool {metadata['name']!r} did not become available within {timeout} seconds"
                )
            time.sleep(2)

    def wait_service_ready(
        self, sandbox: dict[str, Any], service: str, time_to_start: Optional[float] = None
    ) -> None:
        client = self.service_client(sandbox, service)
        timeout = time_to_start if time_to_start is not None else 600.0
        deadline = time.monotonic() + timeout
        try:
            while True:
                response = client.get("/status")
                if response.status_code not in {502, 503, 504}:
                    response.raise_for_status()
                    return
                if time.monotonic() >= deadline:
                    raise TimeoutError(
                        f"Fleet service {service!r} did not become ready within {timeout} seconds"
                    )
                time.sleep(2)
        finally:
            client.close()

    def service_client(self, sandbox: dict[str, Any], service: str) -> httpx.Client:
        if service not in sandbox["services"]:
            raise ValueError(f"Fleet sandbox does not expose service {service!r}")
        return httpx.Client(
            base_url=f"{self._base_url}/api/svc/{sandbox['namespace']}/{sandbox['sandbox']}-{service}/",
            headers=self._headers,
            timeout=30.0,
        )

    def service_url(self, sandbox: dict[str, Any], service: str) -> str:
        if service not in sandbox["services"]:
            raise ValueError(f"Fleet sandbox does not expose service {service!r}")
        return f"{self._base_url}/api/svc/{sandbox['namespace']}/{sandbox['sandbox']}-{service}/"

    def get_pool(self, name: str) -> dict[str, Any]:
        response = httpx.get(self._pool_url(name), headers=self._headers, timeout=30.0)
        response.raise_for_status()
        return response.json()

    def get_claim(self, pool: dict[str, Any]) -> dict[str, Any]:
        metadata = pool["metadata"]
        response = httpx.get(
            f"{self._claim_url(metadata['namespace'])}/{metadata['name']}-claim",
            headers=self._headers,
            timeout=30.0,
        )
        response.raise_for_status()
        return response.json()

    def list_pools(self) -> list[dict[str, Any]]:
        response = httpx.get(
            f"{self._base_url}/api/k8s/apis/cua.ai/v1/osgymworkspacepools",
            headers=self._headers,
            timeout=30.0,
        )
        response.raise_for_status()
        payload = response.json()
        return payload if isinstance(payload, list) else payload.get("items", [])

    def set_pool_replicas(self, pool: dict[str, Any], replicas: int) -> dict[str, Any]:
        metadata = pool["metadata"]
        response = httpx.patch(
            f"{self._pool_url(metadata['namespace'])}/{metadata['name']}",
            json={"spec": {"replicas": replicas}},
            headers={**self._headers, "Content-Type": "application/merge-patch+json"},
            timeout=30.0,
        )
        response.raise_for_status()
        return response.json()

    def _pool_url(self, namespace: str) -> str:
        return f"{self._base_url}/api/k8s/apis/cua.ai/v1/namespaces/{namespace}/osgymworkspacepools"

    def _claim_url(self, namespace: str) -> str:
        return f"{self._base_url}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/{namespace}/osgymsandboxclaims"


class FleetCloudTransport(FleetTransport):
    """Provision and manage registry-image sandboxes through Fleet."""

    def __init__(
        self,
        *,
        image: Optional[Image],
        name: str,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
    ) -> None:
        if disk_gb is not None:
            raise ValueError("disk_gb is not supported by the Fleet cloud transport")
        if region != "us-east-1":
            raise ValueError("Fleet cloud sandboxes currently support only region='us-east-1'")
        self._image = image
        self._name = name
        self._cpu = cpu
        self._memory_mb = memory_mb
        self._time_to_start = time_to_start if time_to_start is not None else 600.0
        self._request_timeout = request_timeout or 30.0
        self._provisioned = False
        self._owns_resources = image is not None
        self._pool: Any = None
        self._claim: Any = None
        self._sdk: Any = None
        self._call_lock = threading.Lock()

    @property
    def name(self) -> str:
        return self._name

    async def connect(self) -> None:
        if not self._provisioned:
            if self._sdk is None:
                self._sdk = await asyncio.to_thread(_FleetClient)
            try:
                if self._pool is None:
                    if self._image is None:
                        self._pool = await self._run(self._sdk.get_pool, self._name)
                    else:
                        self._validate_image(self._image)
                        self._pool = await self._run(self._sdk.create_pool, self._pool_request())
                if self._claim is None:
                    if self._image is None:
                        self._claim = await self._run(self._sdk.get_claim, self._pool)
                    else:
                        self._claim = await self._run(
                            self._sdk.create_claim,
                            {"pool": self._pool, "time_to_start": self._time_to_start},
                        )
                bound = await self._run(
                    self._sdk.wait_claim,
                    self._claim,
                    services=self._service_names(self._pool),
                    time_to_start=self._time_to_start,
                )
                await self._run(
                    self._sdk.wait_service_ready,
                    bound,
                    "server",
                    self._time_to_start,
                )
            except BaseException as provisioning_error:
                try:
                    if self._owns_resources:
                        await self._cleanup_resources()
                except BaseException as cleanup_error:
                    logger.warning(
                        "Failed to clean up Fleet sandbox %r after provisioning failure: %s",
                        self._name,
                        cleanup_error,
                    )
                    raise provisioning_error from cleanup_error
                raise
            FleetTransport.__init__(
                self,
                sdk=self._sdk,
                bound=bound,
                service_name="server",
                timeout=self._request_timeout,
                call_lock=self._call_lock,
            )
            self._provisioned = True
        await FleetTransport.connect(self)

    async def create_snapshot(self, **_: Any) -> dict[str, Any]:
        """Fail clearly because Fleet does not expose snapshot operations."""
        raise NotImplementedError("Snapshots are not supported by the Fleet cloud transport")

    async def forward_tunnel(self, sandbox_port: int | str) -> "TunnelInfo":
        """Return the authenticated Fleet service endpoint for an exposed TCP port."""
        if not isinstance(sandbox_port, int):
            raise ValueError("Fleet services can only expose numeric TCP ports")
        service = "server" if sandbox_port == 8000 else f"port-{sandbox_port}"
        if not self._provisioned:
            raise ValueError("Transport not connected")
        from cua_sandbox.interfaces.tunnel import TunnelInfo

        endpoint = await self._run(self._sdk.service_url, self._bound, service)
        parsed = urlparse(endpoint)
        return TunnelInfo(
            parsed.hostname or "",
            parsed.port or (443 if parsed.scheme == "https" else 80),
            sandbox_port,
            url=endpoint,
        )

    async def delete_vm(self) -> None:
        await self._cleanup_resources()

    async def _cleanup_resources(self) -> None:
        if self._claim is not None:
            await self._run(self._sdk.delete_claim, self._claim)
            self._claim = None
        if self._pool is not None:
            await self._run(self._sdk.delete_pool, self._pool)
            self._pool = None
        self._provisioned = False

    @classmethod
    async def list_sandboxes(cls) -> list[dict[str, Any]]:
        sdk = await asyncio.to_thread(_FleetClient)
        return await asyncio.to_thread(sdk.list_pools)

    @classmethod
    async def get_sandbox_info(cls, name: str) -> dict[str, Any]:
        sdk = await asyncio.to_thread(_FleetClient)
        return await asyncio.to_thread(sdk.get_pool, name)

    @classmethod
    async def suspend_sandbox(cls, name: str) -> None:
        sdk = await asyncio.to_thread(_FleetClient)
        pool = await asyncio.to_thread(sdk.get_pool, name)
        await asyncio.to_thread(sdk.set_pool_replicas, pool, 0)

    @classmethod
    async def resume_sandbox(cls, name: str, time_to_start: Optional[float] = None) -> None:
        sdk = await asyncio.to_thread(_FleetClient)
        pool = await asyncio.to_thread(sdk.get_pool, name)
        await asyncio.to_thread(sdk.set_pool_replicas, pool, 1)
        await asyncio.to_thread(sdk.wait_pool_ready, pool, time_to_start)

    @classmethod
    async def restart_sandbox(cls, name: str, time_to_start: Optional[float] = None) -> None:
        await cls.suspend_sandbox(name)
        await cls.resume_sandbox(name, time_to_start)

    @classmethod
    async def delete_sandbox(cls, name: str) -> None:
        sdk = await asyncio.to_thread(_FleetClient)
        pool = await asyncio.to_thread(sdk.get_pool, name)
        claim = await asyncio.to_thread(sdk.get_claim, pool)
        await asyncio.to_thread(sdk.delete_claim, claim)
        await asyncio.to_thread(sdk.delete_pool, pool)

    def _pool_request(self) -> dict[str, Any]:
        assert self._image is not None
        template: dict[str, Any] = {
            "containerDiskImage": self._image._registry,
            "imagePullSecret": "ecr-credentials",
            "probes": {"readinessProbe": {"tcpSocket": {"port": 8000}}},
        }
        if self._cpu is not None:
            template["cpuCores"] = self._cpu
        if self._memory_mb is not None:
            template["memory"] = f"{self._memory_mb}Mi"
        services = [{"name": "server", "targetPort": 8000, "protocol": "TCP"}]
        services.extend(
            {"name": f"port-{port}", "targetPort": port, "protocol": "TCP"}
            for port in self._image._ports
            if port != 8000
        )
        return {
            "namespace": self._name,
            "spec": {"replicas": 1, "services": services, "template": template},
        }

    @staticmethod
    def _service_names(pool: dict[str, Any]) -> list[str]:
        services = (pool.get("spec") or {}).get("services") or []
        return [service["name"] for service in services if service.get("name")] or ["server"]

    async def _run(self, operation: Any, *args: Any, **kwargs: Any) -> Any:
        return await asyncio.to_thread(self._call, operation, args, kwargs)

    def _call(self, operation: Any, args: tuple[Any, ...], kwargs: dict[str, Any]) -> Any:
        with self._call_lock:
            return operation(*args, **kwargs)

    @staticmethod
    def _validate_image(image: Image) -> None:
        if not image._registry:
            raise ValueError("Fleet cloud sandboxes require Image.from_registry(...)")
        if (
            image._layers
            or image._env
            or image._files
            or image._snapshot_source
            or image._disk_path
        ):
            raise ValueError(
                "Fleet cloud supports registry images with optional exposed services only"
            )
