from __future__ import annotations

from types import SimpleNamespace

import fleet_sdk
import pytest
from cua_sandbox import (
    ClaimSpec,
    CreatePoolRequest,
    CreatePoolRequestBuilder,
    CreateTemplateRequest,
    CreateTemplateRequestBuilder,
    Firmware,
    Image,
    OsGymSandboxTemplateSpecBuilder,
    OsGymSandboxWarmPoolSpecBuilder,
    Pool,
    PoolAccessDeniedError,
    RuntimeKind,
    SandboxServiceBuilder,
    SandboxTemplateRefBuilder,
    ServiceProtocol,
    Template,
    VmTemplate,
    VmTemplateBuilder,
)
from cua_sandbox.pool import _ClaimHandle
from cua_sandbox.sync import Pool as SyncPool
from cua_sandbox.sync import Template as SyncTemplate
from cua_sandbox.transport.fleet_cloud import _FleetClient
from fleet_sdk import Sandbox as FleetSandbox
from fleet_sdk import SdkError, WarmPoolAutoscaling


def test_public_pool_schema_exports_runtime_kind() -> None:
    assert RuntimeKind.KUBEVIRT.value == 0


def test_public_pool_schema_exports_generated_builders() -> None:
    service = SandboxServiceBuilder().name("server").target_port(8000).build()
    vm_template = (
        VmTemplateBuilder()
        .container_disk_image("registry.example/workspace:latest")
        .services([service])
        .build()
    )
    template_request = (
        CreateTemplateRequestBuilder()
        .namespace("default")
        .name("workspace")
        .spec(OsGymSandboxTemplateSpecBuilder().vm_template(vm_template).build())
        .build()
    )
    pool_request = (
        CreatePoolRequestBuilder()
        .namespace("default")
        .spec(
            OsGymSandboxWarmPoolSpecBuilder()
            .replicas(1)
            .sandbox_template_ref(SandboxTemplateRefBuilder().name("workspace").build())
            .build()
        )
        .build()
    )

    assert template_request.spec.vm_template.services == [service]
    assert pool_request.spec.sandbox_template_ref.name == "workspace"


def pool_request(
    *,
    name: str = "foo",
    template_name: str | None = None,
    replicas: int = 1,
) -> CreatePoolRequest:
    template_ref = SandboxTemplateRefBuilder().name(template_name or name).build()
    spec = (
        OsGymSandboxWarmPoolSpecBuilder()
        .replicas(replicas)
        .sandbox_template_ref(template_ref)
        .build()
    )
    return CreatePoolRequestBuilder().namespace(name).spec(spec).build()


def template_request(
    *,
    name: str = "foo",
    image: str = "example:latest",
    services: dict[str, int] | None = None,
    vm_template: VmTemplate | None = None,
) -> CreateTemplateRequest:
    if vm_template is None:
        built_services = [
            SandboxServiceBuilder()
            .name(service_name)
            .target_port(port)
            .protocol(ServiceProtocol.TCP)
            .build()
            for service_name, port in (services or {"server": 8000}).items()
        ]
        vm_template = (
            VmTemplateBuilder()
            .container_disk_image(image)
            .image_pull_secret("ecr-credentials")
            .services(built_services)
            .build()
        )

    spec = OsGymSandboxTemplateSpecBuilder().vm_template(vm_template).build()
    return CreateTemplateRequestBuilder().namespace(name).name(name).spec(spec).build()


def fleet_pool(name: str = "foo") -> SimpleNamespace:
    return SimpleNamespace(
        metadata=SimpleNamespace(name=name, namespace=name),
        spec=SimpleNamespace(
            replicas=1,
            sandbox_template_ref=SimpleNamespace(name=name),
            autoscaling=None,
        ),
        status=SimpleNamespace(replicas=1, ready_replicas=1, selector=None),
    )


def fleet_template(name: str = "foo") -> SimpleNamespace:
    return SimpleNamespace(
        metadata=SimpleNamespace(name=name, namespace=name),
        spec=SimpleNamespace(vm_template=SimpleNamespace(services=[])),
    )


def fake_claim(namespace: str = "foo", name: str = "claim-1") -> SimpleNamespace:
    return SimpleNamespace(metadata=SimpleNamespace(namespace=namespace, name=name))


class FakeFleetClient:
    def __init__(
        self,
        *,
        existing: object | None = None,
        reconcile_error: Exception | None = None,
        release_error: Exception | None = None,
    ):
        self.existing = existing
        self.reconcile_error = reconcile_error
        self.release_error = release_error
        self.reconciled: list[object] = []
        self.reconciled_templates: list[object] = []
        self.claims: list[object] = []
        self.released: list[str] = []
        self.deleted_pools: list[str] = []
        self.deleted_templates: list[str] = []
        self.renewed: list[tuple[str, str]] = []
        self.service_requests: list[object] = []
        self.signed_url_creates: list[tuple[object, str, str | None, int]] = []
        self.signed_url_revocations: list[object] = []
        self.wait_calls = 0
        self.closed = False

    async def reconcile_pool(self, request: object) -> object:
        self.reconciled.append(request)
        if self.reconcile_error:
            raise self.reconcile_error
        return self.existing or fleet_pool(request.namespace)

    async def reconcile_template(self, request: object) -> object:
        self.reconciled_templates.append(request)
        if self.reconcile_error:
            raise self.reconcile_error
        return self.existing or fleet_template(request.name)

    async def get_pool(self, name: str) -> object:
        return self.existing or fleet_pool(name)

    async def create_claim(self, request: object) -> object:
        self.claims.append(request)
        return fake_claim()

    async def wait_claim(self, claim: object) -> FleetSandbox:
        assert claim.metadata.name == "claim-1"
        self.wait_calls += 1
        return FleetSandbox(
            namespace="foo",
            claim="claim-1",
            name="sandbox-1",
            services=["server", "mcp"],
        )

    async def wait_service_ready(self, sandbox, service, time_to_start=None) -> None:
        return None

    async def renew_claim(self, claim: object, shutdown_time: str) -> object:
        self.renewed.append((claim.metadata.name, shutdown_time))
        return claim

    async def service_request(self, sandbox, service, path, request):
        self.service_requests.append((sandbox, service, path, request))
        return SimpleNamespace(status=200, headers=[], body=b'{"result":"ok"}')

    async def create_signed_service_url(
        self,
        sandbox,
        service,
        *,
        label=None,
        expires_in_seconds,
    ):
        self.signed_url_creates.append((sandbox, service, label, expires_in_seconds))
        return signed_service_url()

    async def list_signed_service_urls(self, sandbox):
        return [signed_service_url()]

    async def revoke_signed_service_url(self, signed_url):
        self.signed_url_revocations.append(signed_url)

    async def delete_claim(self, claim: object) -> None:
        self.released.append(claim.metadata.name)
        if self.release_error:
            raise self.release_error

    async def delete_pool(self, pool: object) -> None:
        self.deleted_pools.append(pool.metadata.name)

    async def delete_template(self, template: object) -> None:
        self.deleted_templates.append(template.metadata.name)

    async def close(self) -> None:
        self.closed = True


def signed_service_url() -> SimpleNamespace:
    return SimpleNamespace(
        id="31e1c9bb-8cc9-4c50-9cf4-51798b6978e4",
        namespace="foo",
        claim="claim-1",
        sandbox="sandbox-1",
        service="mcp",
        label="Customer demo",
        url="https://signed.example/link",
        created_at="2026-09-01T12:00:00Z",
        expires_at="2026-09-01T13:00:00Z",
        revoked_at=None,
    )


@pytest.mark.asyncio
async def test_fleet_client_get_pool_uses_name_lookup_without_listing() -> None:
    expected = fleet_pool()

    class GeneratedClient:
        async def get_pool(self, name: str) -> object:
            assert name == "foo"
            return expected

    client = object.__new__(_FleetClient)
    client._client = GeneratedClient()

    async def fail_if_listed() -> list[object]:
        raise AssertionError("get_pool must not list pools")

    client.list_pools = fail_if_listed

    assert await client.get_pool("foo") is expected


@pytest.mark.asyncio
async def test_fleet_client_reconcile_pool_delegates_to_generated_client() -> None:
    expected = fleet_pool()

    class GeneratedClient:
        async def reconcile_pool(self, request: object) -> object:
            assert request == "desired"
            return expected

    client = object.__new__(_FleetClient)
    client._client = GeneratedClient()

    assert await client.reconcile_pool("desired") is expected


@pytest.mark.asyncio
async def test_fleet_client_reconcile_template_delegates_to_generated_client() -> None:
    expected = fleet_template()

    class GeneratedClient:
        async def reconcile_template(self, request: object) -> object:
            assert request == "desired"
            return expected

    client = object.__new__(_FleetClient)
    client._client = GeneratedClient()

    assert await client.reconcile_template("desired") is expected


@pytest.mark.asyncio
async def test_fleet_client_manages_signed_service_urls(monkeypatch) -> None:
    calls: list[tuple[str, object]] = []

    class RequestBuilder:
        def __init__(self):
            self.values = {}

        def sandbox(self, value):
            self.values["sandbox"] = value
            return self

        def service(self, value):
            self.values["service"] = value
            return self

        def label(self, value):
            self.values["label"] = value
            return self

        def expires_in_seconds(self, value):
            self.values["expires_in_seconds"] = value
            return self

        def build(self):
            return SimpleNamespace(**self.values)

    class GeneratedSignedServiceURL(SimpleNamespace):
        def __init__(self, **values):
            super().__init__(**values)

    class GeneratedClient:
        async def create_signed_service_url(self, request):
            calls.append(("create", request))
            return signed_service_url()

        async def list_signed_service_urls(self, sandbox):
            calls.append(("list", sandbox))
            return [signed_service_url()]

        async def revoke_signed_service_url(self, signed_url):
            calls.append(("revoke", signed_url))

    monkeypatch.setattr(
        fleet_sdk,
        "CreateSignedServiceUrlRequestBuilder",
        RequestBuilder,
        raising=False,
    )
    monkeypatch.setattr(fleet_sdk, "SignedServiceUrl", GeneratedSignedServiceURL, raising=False)
    client = object.__new__(_FleetClient)
    client._client = GeneratedClient()
    sandbox = FleetSandbox(
        namespace="foo",
        claim="claim-1",
        name="sandbox-1",
        services=["mcp"],
    )

    created = await client.create_signed_service_url(
        sandbox,
        "mcp",
        label="Customer demo",
        expires_in_seconds=3600,
    )
    listed = await client.list_signed_service_urls(sandbox)
    await client.revoke_signed_service_url(created)

    create_request = calls[0][1]
    assert create_request.sandbox is sandbox
    assert (create_request.service, create_request.label, create_request.expires_in_seconds) == (
        "mcp",
        "Customer demo",
        3600,
    )
    assert calls[1] == ("list", sandbox)
    assert listed == [created]
    assert calls[2][0] == "revoke"
    assert calls[2][1].id == created.id


@pytest.mark.asyncio
async def test_reconcile_creates_pool_referencing_a_template(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    pool = await Pool.reconcile(pool_request(template_name="workspace"))

    assert pool.name == "foo"
    assert pool.resource.metadata.name == "foo"
    assert client.closed is True
    assert len(client.reconciled) == 1
    request = client.reconciled[0]
    assert request.namespace == "foo"
    assert request.spec.sandbox_template_ref.name == "workspace"
    assert request.spec.replicas == 1


@pytest.mark.asyncio
async def test_pool_delete_deletes_resource_and_closes_client(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await Pool(fleet_pool()).delete()

    assert client.deleted_pools == ["foo"]
    assert client.closed is True


@pytest.mark.asyncio
async def test_template_reconcile_creates_template_from_registry_image(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    request = template_request(
        image="registry.example/workspace:latest",
        services={"server": 8000, "port-3000": 3000},
    )
    template = await Template.reconcile(request)

    assert template.name == "foo"
    assert template.resource.metadata.name == "foo"
    assert client.closed is True
    assert client.reconciled_templates == [request]
    vm_template = client.reconciled_templates[0].spec.vm_template
    assert vm_template.container_disk_image == "registry.example/workspace:latest"
    assert [(service.name, service.target_port) for service in vm_template.services] == [
        ("server", 8000),
        ("port-3000", 3000),
    ]


@pytest.mark.asyncio
async def test_template_reconcile_closes_temporary_client_when_reconciliation_fails(monkeypatch):
    client = FakeFleetClient(reconcile_error=RuntimeError("reconcile failed"))
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="reconcile failed"):
        await Template.reconcile(template_request())

    assert client.closed is True


@pytest.mark.parametrize("invalid_value", [None, {}, "not-a-request", pool_request()])
@pytest.mark.asyncio
async def test_template_reconcile_rejects_non_request(invalid_value):
    with pytest.raises(TypeError, match="CreateTemplateRequest"):
        await Template.reconcile(invalid_value)


def test_sync_template_reconcile_matches_async_facade(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    template = SyncTemplate.reconcile(template_request())

    assert template.name == "foo"
    assert client.closed is True


@pytest.mark.asyncio
async def test_reconcile_closes_temporary_client_when_reconciliation_fails(monkeypatch):
    client = FakeFleetClient(reconcile_error=RuntimeError("reconcile failed"))
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="reconcile failed"):
        await Pool.reconcile(pool_request())

    assert client.closed is True


@pytest.mark.asyncio
async def test_reconcile_updates_existing_pool_idempotently(monkeypatch):
    existing = fleet_pool()
    client = FakeFleetClient(existing=existing)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    pool = await Pool.reconcile(pool_request())

    assert pool.resource is existing
    assert len(client.reconciled) == 1
    assert client.reconciled[0].spec.sandbox_template_ref.name == "foo"
    assert client.closed is True


@pytest.mark.asyncio
async def test_claim_releases_claim_and_client_after_block_exception(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    with pytest.raises(RuntimeError, match="body failed"):
        async with pool.claim() as sandbox:
            assert sandbox.name == "sandbox-1"
            raise RuntimeError("body failed")

    assert claim_client.claims[0].pool is pool.resource
    assert claim_client.claims[0].spec is None
    assert claim_client.released == ["claim-1"]
    assert claim_client.closed is True


@pytest.mark.asyncio
async def test_claim_preserves_block_exception_when_release_fails(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient(release_error=RuntimeError("release failed"))
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    with pytest.raises(ValueError, match="body failed"):
        async with pool.claim():
            raise ValueError("body failed")

    assert claim_client.released == ["claim-1"]
    assert claim_client.closed is True


@pytest.mark.asyncio
async def test_claim_raises_release_failure_without_block_exception(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient(release_error=RuntimeError("release failed"))
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    with pytest.raises(RuntimeError, match="release failed"):
        async with pool.claim():
            pass

    assert claim_client.closed is True


@pytest.mark.parametrize("invalid_value", [None, {}, "not-a-request"])
@pytest.mark.asyncio
async def test_reconcile_rejects_non_request(invalid_value):
    with pytest.raises(TypeError, match="CreatePoolRequest"):
        await Pool.reconcile(invalid_value)


def test_sync_pool_delete_matches_async_facade(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    SyncPool(Pool(fleet_pool())).delete()

    assert client.deleted_pools == ["foo"]
    assert client.closed is True


def test_sync_pool_matches_blocking_context_manager(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))

    pool = SyncPool.reconcile(pool_request())
    spec = ClaimSpec(
        sandbox_template_ref=SandboxTemplateRefBuilder().name(pool.name).build(),
        warmpool=None,
        bind_deadline=900,
        lifecycle=None,
    )
    with pool.claim(spec=spec) as sandbox:
        assert sandbox.name == "sandbox-1"

    assert claim_client.claims[0].spec is spec
    assert claim_client.released == ["claim-1"]
    assert claim_client.closed is True


@pytest.mark.asyncio
async def test_reconcile_preserves_replicas_and_template_reference(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await Pool.reconcile(pool_request(replicas=2, template_name="workspace"))

    request = client.reconciled[0]
    assert request.spec.replicas == 2
    assert request.spec.sandbox_template_ref.name == "workspace"


@pytest.mark.asyncio
async def test_template_reconcile_preserves_named_services(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await Template.reconcile(
        template_request(
            image="registry.example/workspace:latest",
            services={"server": 8000, "mcp": 3000},
        )
    )

    vm_template = client.reconciled_templates[0].spec.vm_template
    assert [(service.name, service.target_port) for service in vm_template.services] == [
        ("server", 8000),
        ("mcp", 3000),
    ]


@pytest.mark.asyncio
async def test_reconcile_forwards_native_create_pool_request_unchanged(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)
    request = pool_request(replicas=2, template_name="workspace")

    await Pool.reconcile(request)

    assert client.reconciled == [request]


@pytest.mark.asyncio
async def test_template_reconcile_forwards_native_request_unchanged(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)
    request = template_request(
        vm_template=(
            VmTemplateBuilder()
            .container_disk_image("registry.example/workspace:latest")
            .runtime(RuntimeKind.KUBEVIRT)
            .image_pull_secret("workspace-pull")
            .cpu_cores(10)
            .memory("20Gi")
            .firmware(Firmware.EFI)
            .services(
                [
                    SandboxServiceBuilder()
                    .name("server")
                    .target_port(8000)
                    .protocol(ServiceProtocol.TCP)
                    .build()
                ]
            )
            .build()
        ),
    )

    await Template.reconcile(request)

    assert client.reconciled_templates == [request]


@pytest.mark.asyncio
async def test_claim_exposes_named_service_requests(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    async with pool.claim() as sandbox:
        response = await sandbox.services.request(
            "mcp",
            method="POST",
            path="/mcp",
            json={"jsonrpc": "2.0", "method": "tools/list", "id": 1},
        )

    assert response.status_code == 200
    _, service, path, request = claim_client.service_requests[0]
    assert (service, path, request.method) == ("mcp", "/mcp", "POST")
    assert request.body == b'{"jsonrpc": "2.0", "method": "tools/list", "id": 1}'


@pytest.mark.asyncio
async def test_claim_manages_signed_service_urls(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    async with pool.claim() as sandbox:
        created = await sandbox.services.create_signed_url(
            "mcp",
            label="Customer demo",
            expires_in_seconds=3600,
        )
        listed = await sandbox.services.list_signed_urls()
        await sandbox.services.revoke_signed_url(created)

    bound, service, label, expires_in_seconds = claim_client.signed_url_creates[0]
    assert (bound.name, service, label, expires_in_seconds) == (
        "sandbox-1",
        "mcp",
        "Customer demo",
        3600,
    )
    assert created.url == "https://signed.example/link"
    assert listed == [created]
    assert claim_client.signed_url_revocations == [created]


@pytest.mark.asyncio
async def test_claim_forwards_native_claim_spec_unchanged(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    spec = ClaimSpec(
        sandbox_template_ref=SandboxTemplateRefBuilder().name(pool.name).build(),
        warmpool=None,
        bind_deadline=900,
        lifecycle=None,
    )

    async with pool.claim(spec=spec) as sandbox:
        assert sandbox.name == "sandbox-1"

    request = claim_client.claims[0]
    assert request.pool is pool.resource
    assert request.spec is spec


@pytest.mark.asyncio
async def test_claim_without_spec_uses_operator_default(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    async with pool.claim() as sandbox:
        assert sandbox.name == "sandbox-1"

    request = claim_client.claims[0]
    assert request.pool is pool.resource
    assert request.spec is None
    assert claim_client.released == ["claim-1"]
    assert claim_client.closed is True


@pytest.mark.asyncio
async def test_pool_get_fetches_without_reconciling(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    pool = await Pool.get("foo")

    assert pool.name == "foo"
    assert client.reconciled == []
    assert client.closed is True


@pytest.mark.asyncio
async def test_create_claim_returns_a_serializable_lease(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    lease = await pool.create_claim()

    assert (lease.namespace, lease.name) == ("foo", "claim-1")
    assert claim_client.claims[0].pool is pool.resource
    assert claim_client.claims[0].spec is None
    assert claim_client.released == []
    assert claim_client.closed is True
    assert lease.to_dict() == {
        "version": 1,
        "provider": "fleet",
        "namespace": "foo",
        "pool": "foo",
        "claim": "claim-1",
        "service": "server",
    }
    assert _ClaimHandle.from_dict(lease.to_dict()).to_dict() == lease.to_dict()


@pytest.mark.asyncio
async def test_create_claim_forwards_creation_ttl_in_a_derived_spec(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    await pool.create_claim(ttl_seconds_after_created=1800)

    spec = claim_client.claims[0].spec
    assert spec.ttl_seconds_after_created == 1800
    assert spec.sandbox_template_ref is pool.resource.spec.sandbox_template_ref
    assert spec.warmpool is None
    assert spec.bind_deadline is None
    assert spec.lifecycle is None


@pytest.mark.asyncio
async def test_create_claim_rejects_ttl_alongside_an_explicit_spec(monkeypatch):
    reconcile_client = FakeFleetClient()
    clients = iter([reconcile_client, FakeFleetClient()])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    with pytest.raises(ValueError, match="inside spec"):
        await pool.create_claim(spec=SimpleNamespace(), ttl_seconds_after_created=1800)


@pytest.mark.asyncio
async def test_pool_claim_forwards_creation_ttl_in_a_derived_spec(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    async with pool.claim(ttl_seconds_after_created=900):
        pass

    spec = claim_client.claims[0].spec
    assert spec.ttl_seconds_after_created == 900
    assert spec.sandbox_template_ref is pool.resource.spec.sandbox_template_ref


@pytest.mark.asyncio
async def test_lease_wait_connects_to_the_named_service_and_caches_the_bind(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)
    lease = _ClaimHandle(namespace="foo", name="claim-1", pool_name="foo")

    sandbox = await lease.wait(service="mcp")

    assert sandbox.name == "sandbox-1"
    assert client.wait_calls == 1
    assert lease.to_dict()["claim"] == "claim-1"
    response = await sandbox.services.request(
        "mcp", method="POST", path="/mcp", json={"jsonrpc": "2.0", "id": 1}
    )
    assert response.status_code == 200
    assert client.closed is False
    await sandbox.disconnect()
    assert client.closed is True


@pytest.mark.asyncio
async def test_lease_reattaches_from_serialized_state_without_polling(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)
    lease = _ClaimHandle.from_dict(
        {
            "version": 1,
            "provider": "fleet",
            "namespace": "foo",
            "pool": "foo",
            "claim": "claim-1",
        }
    )

    sandbox = await lease.wait(service="mcp")

    assert sandbox.name == "sandbox-1"
    assert client.wait_calls == 1
    await sandbox.disconnect()
    assert client.closed is True


@pytest.mark.asyncio
async def test_lease_wait_closes_its_client_when_the_bind_fails(monkeypatch):
    client = FakeFleetClient()

    async def failing_wait(claim):
        raise RuntimeError("bind failed")

    client.wait_claim = failing_wait
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="bind failed"):
        await _ClaimHandle(namespace="foo", name="claim-1", pool_name="foo").wait()

    assert client.closed is True


@pytest.mark.asyncio
async def test_lease_release_deletes_the_claim_by_reference(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await _ClaimHandle(namespace="foo", name="claim-1", pool_name="foo").release()

    assert client.released == ["claim-1"]
    assert client.closed is True


@pytest.mark.asyncio
async def test_release_tolerates_claim_already_deleted_by_another_process(monkeypatch):
    first_client = FakeFleetClient()
    second_client = FakeFleetClient()

    async def already_deleted(claim):
        raise SdkError.Status("delete claim", 404, b"not found")

    second_client.delete_claim = already_deleted
    clients = iter([first_client, second_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))

    await _ClaimHandle(namespace="foo", name="claim-1", pool_name="foo").release()
    await _ClaimHandle(namespace="foo", name="claim-1", pool_name="foo").release()

    assert first_client.released == ["claim-1"]
    assert first_client.closed is True
    assert second_client.closed is True


@pytest.mark.asyncio
async def test_lease_renew_pushes_the_shutdown_time_forward(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await _ClaimHandle(namespace="foo", name="claim-1", pool_name="foo").renew(
        "2026-01-01T00:10:00Z"
    )

    assert client.renewed == [("claim-1", "2026-01-01T00:10:00Z")]
    assert client.closed is True


@pytest.mark.asyncio
async def test_fleet_client_renew_claim_requires_a_release_with_renew_support():
    client = object.__new__(_FleetClient)
    client._client = object()

    with pytest.raises(RuntimeError, match="renew_claim"):
        await client.renew_claim(fake_claim(), "2026-01-01T00:10:00Z")


@pytest.mark.asyncio
async def test_fleet_client_renew_claim_delegates_when_supported():
    calls = []

    class GeneratedClient:
        async def renew_claim(self, claim, shutdown_time):
            calls.append((claim, shutdown_time))
            return "renewed"

    client = object.__new__(_FleetClient)
    client._client = GeneratedClient()

    assert await client.renew_claim("claim", "2026-01-01T00:10:00Z") == "renewed"
    assert calls == [("claim", "2026-01-01T00:10:00Z")]


@pytest.mark.asyncio
async def test_named_claim_retry_reattaches_without_creating(monkeypatch):
    client = FakeFleetClient()
    client.existing_claims = [fake_claim()]

    async def list_claims(namespace: str):
        assert namespace == "foo"
        return client.existing_claims

    client.list_claims = list_claims
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    sandbox = await Pool(fleet_pool()).claim(name="claim-1")

    assert client.claims == []
    assert sandbox.claim_name == "claim-1"
    assert sandbox.to_dict()["service"] == "server"
    await sandbox.close()


@pytest.mark.asyncio
async def test_named_claim_reattach_failure_does_not_release_existing_claim(monkeypatch):
    client = FakeFleetClient()
    client.existing_claims = [fake_claim()]

    async def list_claims(namespace: str):
        assert namespace == "foo"
        return client.existing_claims

    async def failing_wait(claim):
        raise RuntimeError("bind failed")

    client.list_claims = list_claims
    client.wait_claim = failing_wait
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="bind failed"):
        await Pool(fleet_pool()).claim(name="claim-1")

    assert client.claims == []
    assert client.released == []
    assert client.closed is True


@pytest.mark.asyncio
async def test_new_claim_acquisition_failure_releases_created_claim(monkeypatch):
    client = FakeFleetClient()

    async def list_claims(namespace: str):
        assert namespace == "foo"
        return []

    async def failing_wait(claim):
        raise RuntimeError("bind failed")

    client.list_claims = list_claims
    client.wait_claim = failing_wait
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="bind failed"):
        await Pool(fleet_pool()).claim(name="claim-1")

    assert len(client.claims) == 1
    assert client.released == ["claim-1"]
    assert client.closed is True


@pytest.mark.asyncio
async def test_pool_apply_delegates_to_template_and_pool_reconcile(monkeypatch):
    template_requests = []
    pool_requests = []
    reconcile_order = []

    async def reconcile_template(cls, request):
        reconcile_order.append("template")
        template_requests.append(request)
        return Template(fleet_template(request.name))

    async def reconcile_pool(cls, request):
        reconcile_order.append("pool")
        pool_requests.append(request)
        return Pool(fleet_pool(request.namespace))

    monkeypatch.setattr(Template, "reconcile", classmethod(reconcile_template))
    monkeypatch.setattr(Pool, "reconcile", classmethod(reconcile_pool))

    pool = await Pool.apply(
        Image.from_registry("registry.example/workspace:latest"),
        name="workspace",
        cpu=4,
        memory_mb=4096,
    )

    assert reconcile_order == ["pool", "template"]
    assert len(template_requests) == 1
    assert len(pool_requests) == 1
    assert pool.name == pool_requests[0].namespace == template_requests[0].name


@pytest.mark.asyncio
async def test_pool_apply_delete_removes_owned_pool_and_template(monkeypatch):
    clients = [FakeFleetClient() for _ in range(3)]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    pool = await Pool.apply(
        Image.from_registry("registry.example/workspace:latest"),
        name="workspace",
    )
    await pool.delete()

    assert clients[2].deleted_pools == ["workspace"]
    assert clients[2].deleted_templates == ["workspace"]


@pytest.mark.asyncio
async def test_pool_apply_rolls_back_pool_when_template_reconcile_fails(monkeypatch):
    clients = [
        FakeFleetClient(),
        FakeFleetClient(reconcile_error=RuntimeError("template failed")),
        FakeFleetClient(),
    ]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    with pytest.raises(RuntimeError, match="template failed"):
        await Pool.apply(
            Image.from_registry("registry.example/workspace:latest"),
            name="workspace",
        )

    assert clients[2].deleted_pools == ["workspace"]
    assert clients[2].deleted_templates == []


@pytest.mark.asyncio
async def test_pool_apply_forwards_autoscaling_to_the_pool_request(monkeypatch):
    clients = [FakeFleetClient() for _ in range(2)]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))
    autoscaling = WarmPoolAutoscaling(min_pool_size=0, initial_pool_size=2, max_pool_size=10)

    await Pool.apply(
        Image.from_registry("registry.example/workspace:latest"),
        name="workspace",
        autoscaling=autoscaling,
    )

    assert clients[0].reconciled[0].spec.autoscaling == autoscaling


@pytest.mark.asyncio
async def test_pool_apply_without_autoscaling_leaves_the_pool_spec_static(monkeypatch):
    clients = [FakeFleetClient() for _ in range(2)]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    await Pool.apply(
        Image.from_registry("registry.example/workspace:latest"),
        name="workspace",
    )

    assert clients[0].reconciled[0].spec.autoscaling is None


@pytest.mark.asyncio
async def test_pool_apply_requires_an_explicit_name():
    image = Image.from_registry("registry.example/workspace:latest")

    with pytest.raises(TypeError):
        await Pool.apply(image)
    with pytest.raises(ValueError, match="globally unique"):
        await Pool.apply(image, name="")
    with pytest.raises(ValueError, match="globally unique"):
        await Pool.apply(image, name=None)


def test_sync_pool_apply_forwards_autoscaling(monkeypatch):
    clients = [FakeFleetClient() for _ in range(2)]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))
    autoscaling = WarmPoolAutoscaling(min_pool_size=1, initial_pool_size=1, max_pool_size=5)

    pool = SyncPool.apply(
        Image.from_registry("registry.example/workspace:latest"),
        name="workspace",
        autoscaling=autoscaling,
    )

    assert pool.name == "workspace"
    assert clients[0].reconciled[0].spec.autoscaling == autoscaling


@pytest.mark.asyncio
async def test_pool_apply_forwards_creation_ttl_to_the_pool_request(monkeypatch):
    clients = [FakeFleetClient() for _ in range(2)]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    await Pool.apply(
        Image.from_registry("registry.example/workspace:latest"),
        name="workspace",
        ttl_seconds_after_created=86400,
    )

    assert clients[0].reconciled[0].spec.ttl_seconds_after_created == 86400


@pytest.mark.asyncio
async def test_pool_apply_without_creation_ttl_leaves_the_pool_unreaped(monkeypatch):
    clients = [FakeFleetClient() for _ in range(2)]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    await Pool.apply(
        Image.from_registry("registry.example/workspace:latest"),
        name="workspace",
    )

    assert clients[0].reconciled[0].spec.ttl_seconds_after_created is None


@pytest.mark.asyncio
async def test_pool_apply_rejects_invalid_creation_ttl(monkeypatch):
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", FakeFleetClient)

    with pytest.raises(ValueError, match="ttl_seconds_after_created"):
        await Pool.apply(
            Image.from_registry("registry.example/workspace:latest"),
            name="workspace",
            ttl_seconds_after_created=-1,
        )


def _forbidden(operation: str) -> SdkError.Status:
    return SdkError.Status(operation=operation, status=403, body="k8s request is not allowed")


@pytest.mark.asyncio
async def test_pool_apply_maps_forbidden_pool_reconcile_to_access_denied(monkeypatch):
    clients = [FakeFleetClient(reconcile_error=_forbidden("create pool"))]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    with pytest.raises(PoolAccessDeniedError, match="globally unique") as error:
        await Pool.apply(
            Image.from_registry("registry.example/workspace:latest"),
            name="workspace",
        )

    assert "'workspace'" in str(error.value)
    assert isinstance(error.value.__cause__, SdkError.Status)


@pytest.mark.asyncio
async def test_pool_apply_maps_forbidden_template_reconcile_and_still_rolls_back(monkeypatch):
    clients = [
        FakeFleetClient(),
        FakeFleetClient(reconcile_error=_forbidden("update template")),
        FakeFleetClient(),
    ]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    with pytest.raises(PoolAccessDeniedError, match="globally unique"):
        await Pool.apply(
            Image.from_registry("registry.example/workspace:latest"),
            name="workspace",
        )

    assert clients[2].deleted_pools == ["workspace"]


@pytest.mark.asyncio
async def test_pool_apply_canonicalizes_native_pool_access_denied(monkeypatch):
    upstream = getattr(SdkError, "PoolAccessDenied", None)
    if upstream is None:
        pytest.skip("installed cua-fleet predates SdkError.PoolAccessDenied")

    native = upstream(
        operation="create pool",
        namespace="workspace",
        status=403,
        body="k8s request is not allowed",
    )
    clients = [FakeFleetClient(reconcile_error=native)]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    with pytest.raises(PoolAccessDeniedError, match="globally unique") as error:
        await Pool.apply(
            Image.from_registry("registry.example/workspace:latest"),
            name="workspace",
        )

    assert error.value is native
    assert "Fleet denied create pool on pool namespace 'workspace'" in str(error.value)
    assert "https://discord.gg/mVnXXpdE85" in str(error.value)


@pytest.mark.asyncio
async def test_pool_apply_rollback_failure_does_not_mask_template_error(monkeypatch):
    class DeleteDeniedClient(FakeFleetClient):
        async def delete_pool(self, pool: object) -> None:
            raise _forbidden("delete pool")

    clients = [
        FakeFleetClient(),
        FakeFleetClient(reconcile_error=_forbidden("update template")),
        DeleteDeniedClient(),
    ]
    iterator = iter(clients)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(iterator))

    with pytest.raises(PoolAccessDeniedError, match="update template"):
        await Pool.apply(
            Image.from_registry("registry.example/workspace:latest"),
            name="workspace",
        )


@pytest.mark.asyncio
async def test_close_after_disconnect_reopens_client_for_release(monkeypatch):
    claim_client = FakeFleetClient()
    release_client = FakeFleetClient()
    clients = iter([claim_client, release_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))

    sandbox = await Pool(fleet_pool()).claim()
    await sandbox.disconnect()
    await sandbox.close()

    assert claim_client.closed is True
    assert claim_client.released == []
    assert release_client.released == ["claim-1"]
    assert release_client.closed is True
