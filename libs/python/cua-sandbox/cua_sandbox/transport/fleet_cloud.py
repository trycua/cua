"""Fleet-backed implementation of the public cloud sandbox transport."""

from __future__ import annotations

import asyncio
import json
import logging
from typing import TYPE_CHECKING, Any, Optional
from urllib.parse import urlparse

from cyclops_sdk import (
    ClaimSpec,
    CreateClaimRequest,
    CreatePoolRequest,
    CyclopsClient,
    CyclopsConfiguration,
    CyclopsCredentials,
    HttpRequest,
    PoolSpec,
    PoolTemplate,
    PreservedJson,
    SandboxService,
    SandboxTemplateRef,
)
from cua_sandbox._config import (
    get_client_id,
    get_client_secret,
    get_fleet_base_url,
    get_token_url,
)
from cua_sandbox.image import Image
from cua_sandbox.transport.cyclops_http_client import CyclopsHttpClient
from cua_sandbox.transport.fleet import FleetTransport

if TYPE_CHECKING:
    from cua_sandbox.interfaces.tunnel import TunnelInfo

logger = logging.getLogger(__name__)


class _FleetClient:
    """Thin async facade over the generated Cyclops SDK."""

    def __init__(self) -> None:
        client_id = get_client_id()
        client_secret = get_client_secret()
        if not client_id or not client_secret:
            raise ValueError(
                "Fleet cloud sandboxes require CUA_CLIENT_ID and CUA_CLIENT_SECRET, "
                "or cua.configure(client_id=..., client_secret=...)."
            )
        self._base_url = get_fleet_base_url().rstrip("/")
        self._http_client = CyclopsHttpClient()
        configuration = CyclopsConfiguration(
            base_url=self._base_url,
            token_url=get_token_url(),
            credentials=CyclopsCredentials(client_id, client_secret),
            pool_poll_interval_ms=2000,
            pool_poll_limit=300,
            claim_poll_interval_ms=2000,
            claim_poll_limit=300,
        )
        self._client = CyclopsClient.connect(configuration, self._http_client)

    async def close(self) -> None:
        await self._http_client.aclose()

    async def create_pool(self, request: CreatePoolRequest) -> Any:
        return await self._client.create_pool(request)

    async def create_claim(self, request: CreateClaimRequest) -> Any:
        return await self._client.create_claim(request)

    async def wait_claim(self, claim: Any) -> Any:
        return await self._client.wait_claim(claim)

    async def delete_claim(self, claim: Any) -> None:
        await self._client.delete_claim(claim)

    async def delete_pool(self, pool: Any) -> None:
        await self._client.delete_pool(pool)

    async def service_request(self, sandbox: Any, service: str, path: str, request: HttpRequest) -> Any:
        return await self._client.service_request(sandbox, service, path, request)

    async def get_pool(self, name: str) -> Any:
        for pool in await self.list_pools():
            if pool.metadata.name == name:
                return pool
        raise LookupError(f"Fleet pool {name!r} was not found")

    async def get_claim(self, pool: Any) -> Any:
        expected = f"{pool.metadata.name}-claim"
        for claim in await self._client.list_claims(pool.metadata.namespace):
            if claim.metadata.name == expected:
                return claim
        raise LookupError(f"Fleet claim {expected!r} was not found")

    async def list_pools(self) -> list[Any]:
        response = await self._http_client.execute(
            HttpRequest(method="GET", url=f"{self._base_url}/api/namespaces", headers=[], body=None)
        )
        if not 200 <= response.status < 300:
            raise RuntimeError(f"Fleet namespace listing failed with HTTP {response.status}")
        payload = json.loads(response.body)
        items = payload if isinstance(payload, list) else payload.get("items", [])
        namespaces = [
            item if isinstance(item, str) else item.get("name") or item.get("metadata", {}).get("name")
            for item in items
        ]
        pools = await asyncio.gather(
            *(self._client.list_pools(namespace) for namespace in namespaces if namespace)
        )
        return [pool for namespace_pools in pools for pool in namespace_pools]

    async def set_pool_replicas(self, pool: Any, replicas: int) -> Any:
        pool.spec.replicas = replicas
        return await self._client.update_pool(pool)

    async def wait_service_ready(
        self, sandbox: Any, service: str, time_to_start: Optional[float] = None
    ) -> None:
        timeout = time_to_start if time_to_start is not None else 600.0
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            response = await self.service_request(
                sandbox,
                service,
                "/status",
                HttpRequest(method="GET", url="https://service.invalid/status", headers=[], body=None),
            )
            if 200 <= response.status < 500:
                return
            if asyncio.get_running_loop().time() >= deadline:
                raise TimeoutError(f"Fleet service {service!r} did not become ready within {timeout} seconds")
            await asyncio.sleep(2)

    def service_url(self, sandbox: Any, service: str) -> str:
        if service not in sandbox.services:
            raise ValueError(f"Fleet sandbox does not expose service {service!r}")
        return f"{self._base_url}/api/svc/{sandbox.namespace}/{sandbox.name}-{service}/"


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

    @property
    def name(self) -> str:
        return self._name

    async def connect(self) -> None:
        if not self._provisioned:
            if self._sdk is None:
                self._sdk = _FleetClient()
            try:
                if self._pool is None:
                    if self._image is None:
                        self._pool = await self._sdk.get_pool(self._name)
                    else:
                        self._validate_image(self._image)
                        self._pool = await self._sdk.create_pool(self._pool_request())
                if self._claim is None:
                    if self._image is None:
                        self._claim = await self._sdk.get_claim(self._pool)
                    else:
                        self._claim = await self._sdk.create_claim(
                            CreateClaimRequest(
                                pool=self._pool,
                                spec=ClaimSpec(
                                    sandbox_template_ref=SandboxTemplateRef(name=self._pool.metadata.name),
                                    warmpool=None,
                                    bind_deadline=None,
                                    lifecycle=None,
                                ),
                            )
                        )
                bound = await self._sdk.wait_claim(self._claim)
                await self._sdk.wait_service_ready(bound, "server", self._time_to_start)
            except BaseException as provisioning_error:
                try:
                    if self._owns_resources:
                        await self._cleanup_resources()
                except BaseException as cleanup_error:
                    logger.warning("Failed to clean up Fleet sandbox %r: %s", self._name, cleanup_error)
                    raise provisioning_error from cleanup_error
                raise
            FleetTransport.__init__(
                self, sdk=self._sdk, bound=bound, service_name="server", timeout=self._request_timeout
            )
            self._provisioned = True
        await FleetTransport.connect(self)

    async def create_snapshot(self, **_: Any) -> dict[str, Any]:
        raise NotImplementedError("Snapshots are not supported by the Fleet cloud transport")

    async def forward_tunnel(self, sandbox_port: int | str) -> "TunnelInfo":
        if not isinstance(sandbox_port, int):
            raise ValueError("Fleet services can only expose numeric TCP ports")
        if not self._provisioned:
            raise ValueError("Transport not connected")
        service = "server" if sandbox_port == 8000 else f"port-{sandbox_port}"
        from cua_sandbox.interfaces.tunnel import TunnelInfo

        endpoint = self._sdk.service_url(self._bound, service)
        parsed = urlparse(endpoint)
        return TunnelInfo(
            parsed.hostname or "", parsed.port or (443 if parsed.scheme == "https" else 80), sandbox_port, url=endpoint
        )

    async def delete_vm(self) -> None:
        await self._cleanup_resources()

    async def _cleanup_resources(self) -> None:
        if self._claim is not None:
            await self._sdk.delete_claim(self._claim)
            self._claim = None
        if self._pool is not None:
            await self._sdk.delete_pool(self._pool)
            self._pool = None
        self._provisioned = False

    @classmethod
    async def list_sandboxes(cls) -> list[Any]:
        sdk = _FleetClient()
        try:
            return await sdk.list_pools()
        finally:
            await sdk.close()

    @classmethod
    async def get_sandbox_info(cls, name: str) -> Any:
        sdk = _FleetClient()
        try:
            return await sdk.get_pool(name)
        finally:
            await sdk.close()

    @classmethod
    async def suspend_sandbox(cls, name: str) -> None:
        sdk = _FleetClient()
        try:
            await sdk.set_pool_replicas(await sdk.get_pool(name), 0)
        finally:
            await sdk.close()

    @classmethod
    async def resume_sandbox(cls, name: str, time_to_start: Optional[float] = None) -> None:
        del time_to_start
        sdk = _FleetClient()
        try:
            await sdk.set_pool_replicas(await sdk.get_pool(name), 1)
        finally:
            await sdk.close()

    @classmethod
    async def restart_sandbox(cls, name: str, time_to_start: Optional[float] = None) -> None:
        await cls.suspend_sandbox(name)
        await cls.resume_sandbox(name, time_to_start)

    @classmethod
    async def delete_sandbox(cls, name: str) -> None:
        sdk = _FleetClient()
        try:
            pool = await sdk.get_pool(name)
            await sdk.delete_claim(await sdk.get_claim(pool))
            await sdk.delete_pool(pool)
        finally:
            await sdk.close()

    def _pool_request(self) -> CreatePoolRequest:
        assert self._image is not None
        services = [SandboxService(name="server", target_port=8000, protocol="TCP")]
        services.extend(
            SandboxService(name=f"port-{port}", target_port=port, protocol="TCP")
            for port in self._image._ports
            if port != 8000
        )
        return CreatePoolRequest(
            namespace=self._name,
            spec=PoolSpec(
                replicas=1,
                services=services,
                template=PoolTemplate(
                    runtime=None,
                    runtime_class_name=None,
                    node_selector=None,
                    tolerations=None,
                    command=None,
                    container_disk_image=self._image._registry,
                    image_pull_secret="ecr-credentials",
                    cpu_cores=self._cpu,
                    memory=None if self._memory_mb is None else f"{self._memory_mb}Mi",
                    firmware=None,
                    probes=PreservedJson.from_json(json.dumps({"readinessProbe": {"tcpSocket": {"port": 8000}}})),
                    oidc=None,
                ),
                autoscaling=None,
            ),
        )

    @staticmethod
    def _service_names(pool: Any) -> list[str]:
        return [service.name for service in pool.spec.services or []] or ["server"]

    @staticmethod
    def _validate_image(image: Image) -> None:
        if not image._registry:
            raise ValueError("Fleet cloud sandboxes require Image.from_registry(...)")
        if image._layers or image._env or image._files or image._snapshot_source or image._disk_path:
            raise ValueError("Fleet cloud supports registry images with optional exposed services only")
