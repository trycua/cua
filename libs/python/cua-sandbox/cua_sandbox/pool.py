"""Fleet template and pool APIs for reusable cloud sandboxes."""

from __future__ import annotations

import logging
from typing import Any, Callable, Coroutine, Generic, TypeVar, cast

from cua_sandbox.image import Image
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.fleet import FleetTransport
from cua_sandbox.transport.fleet_cloud import (
    _NATIVE_POOL_ACCESS_DENIED,
    FleetCloudTransport,
    _canonicalize_pool_access_denied,
    _FleetClient,
    _pool_access_denied,
    validate_ttl_seconds_after_created,
)
from fleet_sdk import (
    Claim,
    ClaimSpec,
    CreateClaimRequest,
    CreatePoolRequest,
    CreateTemplateRequest,
    ResourceMetadata,
    SandboxTemplateRefBuilder,
    SdkError,
    WarmPoolAutoscaling,
)

_T = TypeVar("_T")
logger = logging.getLogger(__name__)


class _ClaimResult(Generic[_T]):
    """Awaitable claim acquisition that also supports scoped cleanup."""

    def __init__(self, factory: Callable[[], Coroutine[Any, Any, _T]]) -> None:
        self._factory = factory
        self._instance: Any = None

    def __await__(self) -> Any:
        return self._factory().__await__()

    async def __aenter__(self) -> _T:
        self._instance = await self._factory()
        return self._instance

    async def __aexit__(self, exc_type: Any, exc: Any, traceback: Any) -> None:
        if self._instance is None:
            return
        try:
            await self._instance.close()
        except BaseException:
            if exc_type is None:
                raise
            logger.exception("Failed to release Fleet claim after an earlier error")


class Template:
    """A reconciled Fleet sandbox template."""

    def __init__(self, resource: Any) -> None:
        self._resource = resource

    @property
    def name(self) -> str:
        return cast(str, self._resource.metadata.name)

    @property
    def resource(self) -> Any:
        return self._resource

    @classmethod
    async def reconcile(cls, request: CreateTemplateRequest) -> "Template":
        if not isinstance(request, CreateTemplateRequest):
            raise TypeError("Template.reconcile requires a CreateTemplateRequest")
        client = _FleetClient()
        try:
            try:
                return cls(await client.reconcile_template(request))
            except _NATIVE_POOL_ACCESS_DENIED as error:
                raise _canonicalize_pool_access_denied(error)
            except SdkError.Status as error:
                if error.status == 403:
                    raise _pool_access_denied(request.namespace, error) from error
                raise
        finally:
            await client.close()


def _claim_stub(namespace: str, name: str) -> Claim:
    return Claim(
        api_version="osgym.cua.ai/v1alpha1",
        kind="OSGymSandboxClaim",
        metadata=ResourceMetadata(
            namespace=namespace,
            name=name,
            labels=None,
            creation_timestamp=None,
        ),
        spec=ClaimSpec(
            sandbox_template_ref=SandboxTemplateRefBuilder().name("").build(),
            warmpool=None,
            bind_deadline=None,
            lifecycle=None,
        ),
        status=None,
    )


class _ClaimHandle:
    """Serializable identity for a held Fleet claim."""

    def __init__(
        self,
        *,
        namespace: str,
        name: str,
        pool_name: str | None = None,
        service: str = "server",
        client: Any = None,
    ) -> None:
        self.namespace = namespace
        self.name = name
        self.pool_name = pool_name or namespace
        self.service = service
        self._client = client

    def to_dict(self) -> dict[str, Any]:
        return {
            "version": 1,
            "provider": "fleet",
            "namespace": self.namespace,
            "pool": self.pool_name,
            "claim": self.name,
            "service": self.service,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "_ClaimHandle":
        if data.get("provider") != "fleet" or data.get("version") != 1:
            raise ValueError("unsupported sandbox reference")
        return cls(
            namespace=data["namespace"],
            pool_name=data["pool"],
            name=data["claim"],
            service=data.get("service", "server"),
        )

    def _operation_client(self) -> tuple[Any, bool]:
        client = self._client
        closed = bool(
            client is not None
            and (getattr(client, "_closed", False) or getattr(client, "closed", False))
        )
        if client is None or closed:
            return _FleetClient(), True
        return client, False

    async def wait(
        self, *, service: str | None = None, time_to_start: float | None = None
    ) -> Sandbox:
        service = service or self.service
        self.service = service
        client, owns_client = self._operation_client()
        self._client = client
        try:
            bound = await client.wait_claim(_claim_stub(self.namespace, self.name))
            if bound.namespace != self.namespace or bound.claim != self.name:
                raise RuntimeError("Fleet returned a sandbox bound to a different claim")
            await client.wait_service_ready(bound, service, time_to_start)
            sandbox = Sandbox(
                FleetTransport(sdk=client, bound=bound, service_name=service, owns_sdk=True),
                name=bound.name,
            )
            sandbox._claim_handle = self
            await sandbox._connect()
            return sandbox
        except BaseException:
            if owns_client:
                if self._client is client:
                    self._client = None
                await client.close()
            raise

    async def renew(self, shutdown_time: str) -> None:
        client, owns_client = self._operation_client()
        try:
            await client.renew_claim(_claim_stub(self.namespace, self.name), shutdown_time)
        finally:
            if owns_client:
                await client.close()

    async def release(self) -> None:
        client, owns_client = self._operation_client()
        try:
            try:
                await client.delete_claim(_claim_stub(self.namespace, self.name))
            except SdkError.Status as error:
                if error.status != 404:
                    raise
        finally:
            if owns_client:
                await client.close()


class Pool:
    """A Fleet warm pool that can provide durable Sandbox claims."""

    def __init__(self, resource: Any, *, owned_template: Any = None) -> None:
        self._resource = resource
        self._owned_template = owned_template

    @property
    def name(self) -> str:
        return cast(str, self._resource.metadata.name)

    @property
    def resource(self) -> Any:
        return self._resource

    @classmethod
    async def reconcile(cls, request: CreatePoolRequest) -> "Pool":
        if not isinstance(request, CreatePoolRequest):
            raise TypeError("Pool.reconcile requires a CreatePoolRequest")
        client = _FleetClient()
        try:
            try:
                return cls(await client.reconcile_pool(request))
            except _NATIVE_POOL_ACCESS_DENIED as error:
                raise _canonicalize_pool_access_denied(error)
            except SdkError.Status as error:
                if error.status == 403:
                    raise _pool_access_denied(request.namespace, error) from error
                raise
        finally:
            await client.close()

    @classmethod
    async def get(cls, name: str) -> "Pool":
        client = _FleetClient()
        try:
            return cls(await client.get_pool(name))
        finally:
            await client.close()

    @classmethod
    async def apply(
        cls,
        image: Image,
        *,
        name: str,
        replicas: int = 1,
        cpu: int | None = None,
        memory_mb: int | None = None,
        services: dict[str, int] | None = None,
        autoscaling: WarmPoolAutoscaling | None = None,
        ttl_seconds_after_created: int | None = None,
    ) -> "Pool":
        if not isinstance(name, str) or not name:
            raise ValueError(
                "Pool.apply requires an explicit non-empty pool name; pool "
                "names are globally unique across accounts"
            )
        FleetCloudTransport._validate_image(image)
        effective_services = services or {
            "server": 8000,
            **{f"port-{port}": port for port in image._ports if port != 8000},
        }
        transport = FleetCloudTransport(
            image=image,
            name=name,
            replicas=replicas,
            cpu=cpu,
            memory_mb=memory_mb,
            services=effective_services,
            autoscaling=autoscaling,
            ttl_seconds_after_created=ttl_seconds_after_created,
        )
        pool = await cls.reconcile(transport._pool_request())
        try:
            template = await Template.reconcile(transport._template_request())
        except BaseException:
            try:
                await pool.delete()
            except Exception:
                logger.exception(
                    "Failed to roll back Fleet pool %r after template reconcile failure", name
                )
            raise
        pool._owned_template = template.resource
        return pool

    async def delete(self) -> None:
        """Delete this Fleet pool."""
        client = _FleetClient()
        try:
            await client.delete_pool(self._resource)
            if self._owned_template is not None:
                await client.delete_template(self._owned_template)
                self._owned_template = None
        finally:
            await client.close()

    def _claim_spec(
        self, spec: ClaimSpec | None, ttl_seconds_after_created: int | None
    ) -> ClaimSpec | None:
        if ttl_seconds_after_created is None:
            return spec
        if spec is not None:
            raise ValueError(
                "pass ttl_seconds_after_created inside spec when supplying an explicit ClaimSpec"
            )
        validate_ttl_seconds_after_created(ttl_seconds_after_created)
        return ClaimSpec(
            sandbox_template_ref=self._resource.spec.sandbox_template_ref,
            warmpool=None,
            bind_deadline=None,
            lifecycle=None,
            ttl_seconds_after_created=ttl_seconds_after_created,
        )

    async def create_claim(
        self,
        *,
        spec: ClaimSpec | None = None,
        name: str | None = None,
        ttl_seconds_after_created: int | None = None,
    ) -> _ClaimHandle:
        spec = self._claim_spec(spec, ttl_seconds_after_created)
        request = CreateClaimRequest(pool=self._resource, spec=spec, name=name)
        client = _FleetClient()
        try:
            claim = await client.create_claim(request)
            return _ClaimHandle(
                namespace=claim.metadata.namespace,
                name=claim.metadata.name,
                pool_name=self.name,
            )
        finally:
            await client.close()

    def claim(
        self,
        *,
        spec: ClaimSpec | None = None,
        name: str | None = None,
        service: str = "server",
        time_to_start: float | None = None,
        ttl_seconds_after_created: int | None = None,
    ) -> _ClaimResult[Sandbox]:
        spec = self._claim_spec(spec, ttl_seconds_after_created)

        async def acquire() -> Sandbox:
            client = _FleetClient()
            claim: Any = None
            created_claim = False
            try:
                if name is not None:
                    claim = next(
                        (
                            existing
                            for existing in await client.list_claims(
                                self._resource.metadata.namespace
                            )
                            if existing.metadata.name == name
                        ),
                        None,
                    )
                if claim is None:
                    claim = await client.create_claim(
                        CreateClaimRequest(pool=self._resource, spec=spec, name=name)
                    )
                    created_claim = True
                handle = _ClaimHandle(
                    namespace=claim.metadata.namespace,
                    name=claim.metadata.name,
                    pool_name=self.name,
                    service=service,
                    client=client,
                )
                return await handle.wait(service=service, time_to_start=time_to_start)
            except BaseException:
                if created_claim and claim is not None:
                    try:
                        await client.delete_claim(claim)
                    except Exception:
                        logger.exception("Failed to release Fleet claim after acquisition failure")
                await client.close()
                raise

        return _ClaimResult(acquire)
