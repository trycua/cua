from __future__ import annotations

from types import SimpleNamespace

import pytest
from cua_sandbox import (
    ClaimLifecycle,
    ClaimSpec,
    CreatePoolRequest,
    CreateTemplateRequest,
    Firmware,
    Lease,
    OsGymSandboxTemplateSpec,
    OsGymSandboxWarmPoolSpec,
    Pool,
    PreservedJson,
    RuntimeKind,
    SandboxService,
    SandboxTemplateRef,
    ServiceProtocol,
    Template,
    VmTemplate,
    supports_claim_renewal,
)
from cua_sandbox.sync import Pool as SyncPool
from cua_sandbox.sync import Template as SyncTemplate
from cua_sandbox.transport.fleet_cloud import _FleetClient
from fleet_sdk import Sandbox as FleetSandbox


def test_public_pool_schema_exports_runtime_kind() -> None:
    assert RuntimeKind.KUBEVIRT.value == 0


def test_public_pool_schema_exports_claim_lifecycle_and_preserved_json() -> None:
    lifecycle = ClaimLifecycle(
        shutdown_time="2026-01-01T00:00:00Z", shutdown_policy=None, auto_renew=False
    )
    assert lifecycle.shutdown_time == "2026-01-01T00:00:00Z"
    assert lifecycle.auto_renew is False
    assert hasattr(PreservedJson, "from_json")


def pool_request(
    *,
    name: str = "foo",
    template_name: str | None = None,
    replicas: int = 1,
) -> CreatePoolRequest:
    return CreatePoolRequest(
        namespace=name,
        spec=OsGymSandboxWarmPoolSpec(
            replicas=replicas,
            sandbox_template_ref=SandboxTemplateRef(name=template_name or name),
            autoscaling=None,
        ),
    )


def template_request(
    *,
    name: str = "foo",
    image: str = "example:latest",
    services: dict[str, int] | None = None,
    vm_template: VmTemplate | None = None,
) -> CreateTemplateRequest:
    return CreateTemplateRequest(
        namespace=name,
        name=name,
        spec=OsGymSandboxTemplateSpec(
            vm_template=vm_template
            or VmTemplate(
                container_disk_image=image,
                command=None,
                runtime=None,
                runtime_class_name=None,
                node_selector=None,
                tolerations=None,
                image_pull_policy=None,
                image_pull_secret="ecr-credentials",
                cpu_cores=None,
                memory=None,
                firmware=None,
                probes=None,
                services=[
                    SandboxService(
                        name=service_name, target_port=port, protocol=ServiceProtocol.TCP
                    )
                    for service_name, port in (services or {"server": 8000}).items()
                ],
                oidc=None,
            ),
        ),
    )


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
        self.renewed: list[tuple[str, str]] = []
        self.service_requests: list[object] = []
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

    async def renew_claim(self, claim: object, shutdown_time: str) -> object:
        self.renewed.append((claim.metadata.name, shutdown_time))
        return claim

    async def service_request(self, sandbox, service, path, request):
        self.service_requests.append((sandbox, service, path, request))
        return SimpleNamespace(status=200, headers=[], body=b'{"result":"ok"}')

    async def delete_claim(self, claim: object) -> None:
        self.released.append(claim.metadata.name)
        if self.release_error:
            raise self.release_error

    async def close(self) -> None:
        self.closed = True


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


def test_sync_pool_matches_blocking_context_manager(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))

    pool = SyncPool.reconcile(pool_request())
    spec = ClaimSpec(
        sandbox_template_ref=SandboxTemplateRef(name=pool.name),
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
        vm_template=VmTemplate(
            container_disk_image="registry.example/workspace:latest",
            command=None,
            runtime=RuntimeKind.KUBEVIRT,
            runtime_class_name=None,
            node_selector=None,
            tolerations=None,
            image_pull_policy=None,
            image_pull_secret="workspace-pull",
            cpu_cores=10,
            memory="20Gi",
            firmware=Firmware.EFI,
            probes=None,
            services=[
                SandboxService(name="server", target_port=8000, protocol=ServiceProtocol.TCP)
            ],
            oidc=None,
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
async def test_claim_forwards_native_claim_spec_unchanged(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile(pool_request())

    spec = ClaimSpec(
        sandbox_template_ref=SandboxTemplateRef(name=pool.name),
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
    assert lease.to_dict() == {"namespace": "foo", "name": "claim-1"}
    assert Lease.from_dict(lease.to_dict()).to_dict() == lease.to_dict()


@pytest.mark.asyncio
async def test_lease_wait_connects_to_the_named_service_and_caches_the_bind(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)
    lease = Lease(namespace="foo", name="claim-1")

    sandbox = await lease.wait(service="mcp")

    assert sandbox.name == "sandbox-1"
    assert client.wait_calls == 1
    assert lease.to_dict()["sandbox"] == {
        "namespace": "foo",
        "claim": "claim-1",
        "name": "sandbox-1",
        "services": ["server", "mcp"],
    }
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
    lease = Lease.from_dict(
        {
            "namespace": "foo",
            "name": "claim-1",
            "sandbox": {
                "namespace": "foo",
                "claim": "claim-1",
                "name": "sandbox-1",
                "services": ["server", "mcp"],
            },
        }
    )

    sandbox = await lease.wait(service="mcp")

    assert sandbox.name == "sandbox-1"
    assert client.wait_calls == 0
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
        await Lease(namespace="foo", name="claim-1").wait()

    assert client.closed is True


@pytest.mark.asyncio
async def test_lease_release_deletes_the_claim_by_reference(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await Lease(namespace="foo", name="claim-1").release()

    assert client.released == ["claim-1"]
    assert client.closed is True


@pytest.mark.asyncio
async def test_lease_renew_pushes_the_shutdown_time_forward(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await Lease(namespace="foo", name="claim-1").renew("2026-01-01T00:10:00Z")

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


def test_supports_claim_renewal_reflects_the_generated_client(monkeypatch):
    class Without:
        pass

    class With:
        async def renew_claim(self, claim, shutdown_time):
            return claim

    monkeypatch.setattr("cua_sandbox.pool.CyclopsClient", Without)
    assert supports_claim_renewal() is False
    monkeypatch.setattr("cua_sandbox.pool.CyclopsClient", With)
    assert supports_claim_renewal() is True


@pytest.mark.asyncio
async def test_create_claim_passes_a_client_supplied_name_when_supported(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))

    class NamedCreateClaimRequest:
        def __init__(self, *, pool, spec, name=None):
            self.pool = pool
            self.spec = spec
            self.name = name

    monkeypatch.setattr("cua_sandbox.pool.CreateClaimRequest", NamedCreateClaimRequest)
    pool = await Pool.reconcile(pool_request())

    await pool.create_claim(name="claim-pinned")

    assert claim_client.claims[0].name == "claim-pinned"
    assert claim_client.closed is True


@pytest.mark.asyncio
async def test_create_claim_names_require_a_fleet_release_with_the_name_field(monkeypatch):
    reconcile_client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: reconcile_client)

    class LegacyCreateClaimRequest:
        def __init__(self, *, pool, spec):
            self.pool = pool
            self.spec = spec

    monkeypatch.setattr("cua_sandbox.pool.CreateClaimRequest", LegacyCreateClaimRequest)
    pool = await Pool.reconcile(pool_request())

    with pytest.raises(RuntimeError, match="claim names"):
        await pool.create_claim(name="claim-pinned")
