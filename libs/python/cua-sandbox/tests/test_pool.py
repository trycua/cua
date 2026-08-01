from __future__ import annotations

from types import SimpleNamespace

import pytest
from cua_sandbox import Image, Pool
from cua_sandbox.sync import Pool as SyncPool
from cua_sandbox.transport.fleet_cloud import _FleetClient
from cyclops_sdk import Sandbox as FleetSandbox


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
        create_error: Exception | None = None,
        release_error: Exception | None = None,
    ):
        self.existing = existing
        self.create_error = create_error
        self.release_error = release_error
        self.created: list[object] = []
        self.updated: list[object] = []
        self.claims: list[object] = []
        self.released: list[object] = []
        self.closed = False

    async def get_pool(self, name: str) -> object:
        if self.existing is None:
            raise LookupError(name)
        return self.existing

    async def create_pool(self, request: object) -> object:
        self.created.append(request)
        if self.create_error:
            raise self.create_error
        return fleet_pool(request.namespace)

    async def update_pool(self, pool: object) -> object:
        self.updated.append(pool)
        return pool

    async def create_claim(self, request: object) -> object:
        self.claims.append(request)
        return "claim-1"

    async def wait_claim(self, claim: object) -> FleetSandbox:
        assert claim == "claim-1"
        return FleetSandbox(namespace="foo", claim="claim-1", name="sandbox-1", services=["server"])

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
    assert len(client.created) == 1
    request = client.created[0]
    assert request.namespace == "foo"
    assert request.spec.template.container_disk_image == "registry.example/workspace:latest"
    assert [(service.name, service.target_port) for service in request.spec.services] == [
        ("server", 8000),
        ("port-3000", 3000),
    ]


@pytest.mark.asyncio
async def test_reconcile_closes_temporary_client_when_creation_fails(monkeypatch):
    client = FakeFleetClient(create_error=RuntimeError("create failed"))
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    with pytest.raises(RuntimeError, match="create failed"):
        await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

    assert client.closed is True


@pytest.mark.asyncio
async def test_reconcile_updates_existing_pool_idempotently(monkeypatch):
    existing = fleet_pool()
    client = FakeFleetClient(existing=existing)
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)

    pool = await Pool.reconcile({"name": "foo", "image": Image.from_registry("example:latest")})

    assert pool.resource is existing
    assert client.created == []
    assert client.updated == [existing]
    assert existing.spec.template.container_disk_image == "example:latest"
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
