from __future__ import annotations

from types import SimpleNamespace

import pytest
from cua_sandbox import Image, Pool
from cua_sandbox.sync import Pool as SyncPool
from cua_sandbox.transport.fleet_cloud import _FleetClient
from fleet_sdk import Sandbox as FleetSandbox


def fleet_pool(name: str = "foo") -> SimpleNamespace:
    return SimpleNamespace(
        metadata=SimpleNamespace(name=name, namespace=name),
        spec=SimpleNamespace(services=[]),
    )


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
        self.claims: list[object] = []
        self.released: list[object] = []
        self.service_requests: list[object] = []
        self.closed = False

    async def reconcile_pool(self, request: object) -> object:
        self.reconciled.append(request)
        if self.reconcile_error:
            raise self.reconcile_error
        return self.existing or fleet_pool(request.namespace)

    async def create_claim(self, request: object) -> object:
        self.claims.append(request)
        return "claim-1"

    async def wait_claim(self, claim: object) -> FleetSandbox:
        assert claim == "claim-1"
        return FleetSandbox(
            namespace="foo",
            claim="claim-1",
            name="sandbox-1",
            services=["server", "mcp"],
        )

    async def service_request(self, sandbox, service, path, request):
        self.service_requests.append((sandbox, service, path, request))
        return SimpleNamespace(status=200, headers=[], body=b'{"result":"ok"}')

    async def delete_claim(self, claim: object) -> None:
        self.released.append(claim)
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
async def test_reconcile_creates_pool_from_registry_image(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    pool = await Pool.reconcile(
        {
            "name": "foo",
            "image": Image.from_registry("registry.example/workspace:latest").expose(3000),
        }
    )

    assert pool.name == "foo"
    assert pool.resource.metadata.name == "foo"
    assert client.closed is True
    assert len(client.reconciled) == 1
    request = client.reconciled[0]
    assert request.namespace == "foo"
    assert request.spec.template.container_disk_image == "registry.example/workspace:latest"
    assert [(service.name, service.target_port) for service in request.spec.services] == [
        ("server", 8000),
        ("port-3000", 3000),
    ]


@pytest.mark.asyncio
async def test_reconcile_closes_temporary_client_when_reconciliation_fails(monkeypatch):
    client = FakeFleetClient(reconcile_error=RuntimeError("reconcile failed"))
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="reconcile failed"):
        await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

    assert client.closed is True


@pytest.mark.asyncio
async def test_reconcile_updates_existing_pool_idempotently(monkeypatch):
    existing = fleet_pool()
    client = FakeFleetClient(existing=existing)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    pool = await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

    assert pool.resource is existing
    assert len(client.reconciled) == 1
    assert client.reconciled[0].spec.template.container_disk_image == "example:latest"
    assert client.closed is True


@pytest.mark.asyncio
async def test_claim_releases_claim_and_client_after_block_exception(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

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
    pool = await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

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
    pool = await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

    with pytest.raises(RuntimeError, match="release failed"):
        async with pool.claim():
            pass

    assert claim_client.closed is True


@pytest.mark.parametrize(
    "config, message",
    [
        ({"image": Image.from_registry("example:latest")}, "name"),
        ({"name": "foo"}, "image"),
        ({"name": "foo", "image": "example:latest"}, "Image.from_registry"),
        ({"name": "foo", "image": Image.linux()}, "Image.from_registry"),
    ],
)
@pytest.mark.asyncio
async def test_reconcile_rejects_invalid_config(config, message):
    with pytest.raises((TypeError, ValueError), match=message):
        await Pool.reconcile(config)


def test_sync_pool_matches_blocking_context_manager(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))

    pool = SyncPool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})
    with pool.claim() as sandbox:
        assert sandbox.name == "sandbox-1"

    assert claim_client.released == ["claim-1"]
    assert claim_client.closed is True


@pytest.mark.asyncio
async def test_reconcile_preserves_replicas_and_named_services(monkeypatch):
    client = FakeFleetClient()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    await Pool.reconcile(
        {
            "name": "foo",
            "image": Image.from_registry("registry.example/workspace:latest"),
            "replicas": 2,
            "services": {"server": 8000, "mcp": 3000},
        }
    )

    request = client.reconciled[0]
    assert request.spec.replicas == 2
    assert [(service.name, service.target_port) for service in request.spec.services] == [
        ("server", 8000),
        ("mcp", 3000),
    ]


@pytest.mark.asyncio
async def test_claim_exposes_named_service_requests(monkeypatch):
    reconcile_client = FakeFleetClient()
    claim_client = FakeFleetClient()
    clients = iter([reconcile_client, claim_client])
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: next(clients))
    pool = await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

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
