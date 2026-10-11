"""Fleet template and pool APIs for reusable cloud sandboxes."""

from __future__ import annotations

import logging
import warnings
from dataclasses import replace
from typing import Any, Callable, Coroutine, Generic, Optional, TypeVar, cast

from cua_sandbox._sdk import ENV_SERVICE
from cua_sandbox.image import Image
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.spec import (
    PoolExport,
    PoolOptions,
    SandboxSpec,
    call_native,
    native_fleet,
)
from cua_sandbox.transport.fleet import claim_service_for, fleet_transport_for
from cua_sandbox.transport.fleet_cloud import (
    _NATIVE_POOL_ACCESS_DENIED,
    FleetCloudTransport,
    FleetRuntime,
    _canonicalize_pool_access_denied,
    _FleetClient,
    _pool_access_denied,
    default_server_port,
    default_services,
    resolve_fleet_runtime,
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


async def _native_apply(name: str, spec: Any, options: Any) -> Any:
    """``Fleet.apply`` (the SDK's one pool writer), with the pool-name 403
    raised as ``PoolAccessDeniedError`` like the other pool calls."""
    from cua_sandbox._sdk import native

    n = native()
    try:
        return await call_native(native_fleet().apply(name, spec, options))
    except n.CuaError.Fleet as error:
        if "globally unique" in str(error):
            raise _pool_access_denied(
                name,
                SdkError.Status(operation="apply pool", status=403, body="forbidden"),
            ) from error
        raise


def legacy_parts(
    image: Image,
    *,
    replicas: int = 1,
    cpu: int | None = None,
    memory_mb: int | None = None,
    services: dict[str, int] | None = None,
    autoscaling: Any = None,
    ttl_seconds_after_created: int | None = None,
    runtime: str | None = None,
) -> tuple[SandboxSpec, PoolOptions]:
    """The old ``Pool.apply`` keywords as :class:`SandboxSpec` +
    :class:`PoolOptions`. A declared ``server`` service (an image with its
    own control server) is the readiness probe, as before."""
    from cua_sandbox.options import tcp

    services = dict(services or {})
    spec = SandboxSpec(
        image=image,
        services=services,
        wait_for=tcp("server") if "server" in services else None,
        cpu=cpu,
        memory_mb=memory_mb,
    )
    if autoscaling is not None:
        options = PoolOptions(
            runtime=runtime,
            replicas=(
                autoscaling.initial_pool_size
                if autoscaling.initial_pool_size is not None
                else replicas
            ),
            min_pool_size=(
                autoscaling.min_pool_size if autoscaling.min_pool_size is not None else 0
            ),
            max_pool_size=autoscaling.max_pool_size,
            pool_ttl=ttl_seconds_after_created,
        )
    else:
        options = PoolOptions(
            runtime=runtime, replicas=replicas, pool_ttl=ttl_seconds_after_created
        )
    return spec, options


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


async def _record_pool_image(sandbox: Sandbox, pool: str) -> None:
    """Sets ``sandbox.image_info`` to pool ``pool``'s template image. Never raises."""
    from cua_sandbox import _sdk
    from cua_sandbox.image import ImageInfo

    try:
        info = ImageInfo._from_native(await _sdk.pool_image_info(pool))
    except Exception:  # noqa: BLE001 - informational; never fail a claim
        info = None
    if info is not None:
        sandbox._image_info_fallback = info
        # The SDK handle the transport opens later reports it too.
        transport = getattr(sandbox, "_transport", None)
        if transport is not None and hasattr(transport, "_image_info"):
            transport._image_info = info


class _ClaimHandle:
    """Serializable identity for a held Fleet claim."""

    def __init__(
        self,
        *,
        namespace: str,
        name: str,
        pool_name: str | None = None,
        service: str = ENV_SERVICE,
        client: Any = None,
        agent_type: str | None = None,
        env_token: str | None = None,
    ) -> None:
        # The claim's per-claim env token (never serialized).
        self._env_token = env_token
        self.namespace = namespace
        self.name = name
        self.pool_name = pool_name or namespace
        self.service = service
        self.agent_type = agent_type
        self._client = client

    def to_dict(self) -> dict[str, Any]:
        return {
            "version": 1,
            "provider": "fleet",
            "namespace": self.namespace,
            "pool": self.pool_name,
            "claim": self.name,
            "service": self.service,
            **({"agent_type": self.agent_type} if self.agent_type else {}),
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "_ClaimHandle":
        if data.get("provider") != "fleet" or data.get("version") != 1:
            raise ValueError("unsupported sandbox reference")
        return cls(
            namespace=data["namespace"],
            pool_name=data["pool"],
            name=data["claim"],
            service=data.get("service", ENV_SERVICE),
            agent_type=data.get("agent_type"),
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
            transport_cls = fleet_transport_for(self.agent_type)
            sandbox = Sandbox(
                transport_cls(
                    sdk=client,
                    bound=bound,
                    service_name=service,
                    owns_sdk=True,
                    env_token=self._env_token,
                ),
                name=bound.name,
            )
            sandbox._claim_handle = self
            await sandbox._connect()
            if not getattr(self, "managed", False):
                # A named pool's template image (a managed claim's SDK
                # handle already knows its own).
                await _record_pool_image(sandbox, self.pool_name)
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

    def __init__(
        self, resource: Any, *, owned_template: Any = None, agent_type: str | None = None
    ) -> None:
        self._resource = resource
        self._owned_template = owned_template
        # Guest control-server flavour of the pool's image ("osworld" or None).
        # Claims need it to pick a transport that speaks the guest's API.
        self._agent_type = agent_type

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
    async def apply(cls, *args: Any, **kwargs: Any) -> "Pool":
        """Reconcile a Fleet pool: ``Pool.apply(name, spec, options)``.

        ``spec`` (:class:`~cua_sandbox.SandboxSpec`) is what the pool's
        sandboxes run and ``options`` (:class:`~cua_sandbox.PoolOptions`) how
        the pool keeps capacity for them. The pool, its namespace and its
        template share ``name``, which is globally unique on Fleet. This is
        the SDK's one pool writer (``Fleet.apply``): the image is pinned to
        the variant the runtime runs, a new pool is rolled back if its
        template fails, and a runtime the image cannot run on raises
        ``cua.CuaError.InvalidArgument`` before anything is created.

        The old form ``Pool.apply(image, *, name=..., replicas=..., cpu=...,
        memory_mb=..., services=..., autoscaling=..., ttl_seconds_after_created=...,
        runtime=...)`` still works and is converted to the new one, with a
        ``DeprecationWarning``.
        """
        if (args and isinstance(args[0], Image)) or "image" in kwargs:
            warnings.warn(
                "Pool.apply(image, name=..., ...) is deprecated; use "
                "Pool.apply(name, SandboxSpec(image=...), PoolOptions(...))",
                DeprecationWarning,
                stacklevel=2,
            )
            return await cls._apply_legacy(*args, **kwargs)
        return await cls._apply(*args, **kwargs)

    @classmethod
    async def _apply(
        cls, name: str, spec: SandboxSpec, options: Optional[PoolOptions] = None
    ) -> "Pool":
        if not isinstance(name, str) or not name:
            raise ValueError(
                "Pool.apply requires an explicit non-empty pool name; pool "
                "names are globally unique across accounts"
            )
        if not isinstance(spec, SandboxSpec):
            raise TypeError("Pool.apply(name, spec, options) takes a SandboxSpec")
        options = options or PoolOptions()
        if not isinstance(options, PoolOptions):
            raise TypeError("Pool.apply(name, spec, options) takes a PoolOptions")
        image = spec.image if isinstance(spec.image, Image) else None
        if image is not None:
            FleetCloudTransport._validate_image(image)
            if options.runtime is None or options.runtime in ("gvisor", "kubevirt"):
                # The one runtime/image rule, before anything is created.
                runtime = resolve_fleet_runtime(options.runtime, image)
                options = replace(options, runtime=runtime.name.lower())
        await _native_apply(name, spec.native(), options.native())
        client = _FleetClient()
        try:
            resource = await client.get_pool(name)
            try:
                template = await client.get_template(name, name)
            except Exception:  # noqa: BLE001 - only needed to delete it later
                template = None
        finally:
            await client.close()
        return cls(
            resource,
            owned_template=template,
            agent_type=image._agent_type if image is not None else None,
        )

    @classmethod
    async def _apply_legacy(
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
        runtime: FleetRuntime | None = None,
    ) -> "Pool":
        if not isinstance(name, str) or not name:
            raise ValueError(
                "Pool.apply requires an explicit non-empty pool name; pool "
                "names are globally unique across accounts"
            )
        FleetCloudTransport._validate_image(image)
        # None (spacesd) unless the image runs its own control server
        # (agent_type="osworld" → the OSWorld Flask server on 5000).
        server_port = default_server_port(image)
        effective_services = services or default_services(server_port, image._ports)
        # Validates every argument and resolves the runtime (the one rule)
        # before anything is created.
        transport = FleetCloudTransport(
            image=image,
            name=name,
            replicas=replicas,
            cpu=cpu,
            memory_mb=memory_mb,
            services=effective_services,
            autoscaling=autoscaling,
            ttl_seconds_after_created=ttl_seconds_after_created,
            fleet_runtime=runtime,
            server_port=server_port,
        )
        spec, options = legacy_parts(
            image,
            replicas=replicas,
            cpu=cpu,
            memory_mb=memory_mb,
            services=dict(effective_services),
            autoscaling=autoscaling,
            ttl_seconds_after_created=ttl_seconds_after_created,
            runtime=transport._runtime.name.lower() if transport._runtime is not None else None,
        )
        return await cls._apply(name, spec, options)

    @staticmethod
    async def export(name: str, *, terraform: bool = False) -> "PoolExport | str":
        """Read pool ``name`` back as the shared model (a :class:`PoolExport`
        whose ``spec`` / ``options`` are the native records), or with
        ``terraform=True`` the equivalent Terraform ``fleets_pool`` block
        (attributes the provider lacks yet are commented)."""
        exported = await call_native(native_fleet().export_pool(name))
        if terraform:
            return cast(str, exported.terraform)
        return PoolExport(
            name=name,
            runtime=exported.runtime,
            spec=exported.spec,
            options=exported.options,
            terraform=exported.terraform,
        )

    @staticmethod
    async def check(name: str, spec: SandboxSpec) -> None:
        """Raise :class:`~cua_sandbox.PoolSpecMismatch` (with a readable diff)
        when the set fields of ``spec`` differ from pool ``name``'s
        template."""
        await call_native(native_fleet().check_pool_spec(name, spec.native()))

    @staticmethod
    async def apply_template(name: str, spec: SandboxSpec) -> None:
        """Lay the set fields of ``spec`` over pool ``name``'s template and
        write it (the pool's capacity is kept; a no-op when nothing
        differs)."""
        await call_native(native_fleet().apply_pool_template(name, spec.native()))

    async def _claim_with_token(
        self,
        token: str,
        *,
        name: str | None,
        ttl: int | None,
        service: str,
        time_to_start: float | None,
        agent_type: str | None,
    ) -> Sandbox:
        from cua_sandbox._sdk import native

        bound = await call_native(
            native_fleet().acquire_with(
                self.name,
                native().FleetClaimOptions(name=name, ttl_seconds=ttl, claim_token=token),
            )
        )
        handle = _ClaimHandle(
            namespace=bound.namespace,
            name=bound.claim,
            pool_name=self.name,
            service=service,
            agent_type=agent_type,
            env_token=token,
        )
        try:
            return await handle.wait(service=service, time_to_start=time_to_start)
        except BaseException:
            try:
                await handle.release()
            except Exception:
                logger.exception("Failed to release Fleet claim after acquisition failure")
            raise

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
                agent_type=self._agent_type,
            )
        finally:
            await client.close()

    def claim(
        self,
        *,
        spec: ClaimSpec | None = None,
        name: str | None = None,
        service: str = ENV_SERVICE,
        time_to_start: float | None = None,
        ttl_seconds_after_created: int | None = None,
        agent_type: str | None = None,
        claim_token: str | None = None,
    ) -> _ClaimResult[Sandbox]:
        """Claim a sandbox. ``agent_type="osworld"`` selects the OSWorld transport
        for pools fetched with ``Pool.get`` (pools from ``Pool.apply`` remember it).

        ``claim_token`` (see :func:`generate_claim_token`) is delivered into the
        sandbox at ``/run/cua/env-token`` through the claim's Secret; the pool's
        spec must set ``claim_secrets=True``. The claim waits (at most 90 s
        after it binds) until the sandbox has it, else it is released and
        :class:`~cua_sandbox.ClaimSecretsNotDelivered` is raised."""
        claim_agent_type = agent_type or self._agent_type
        service = claim_service_for(claim_agent_type, service)
        if claim_token is not None:
            if spec is not None:
                raise ValueError("claim_token cannot be combined with an explicit ClaimSpec")
            if ttl_seconds_after_created is not None:
                validate_ttl_seconds_after_created(ttl_seconds_after_created)
            return _ClaimResult(
                lambda: self._claim_with_token(
                    claim_token,
                    name=name,
                    ttl=ttl_seconds_after_created,
                    service=service,
                    time_to_start=time_to_start,
                    agent_type=claim_agent_type,
                )
            )
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
                    agent_type=claim_agent_type,
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
