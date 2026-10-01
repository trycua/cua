from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from cua_sandbox import Image, Pool, Sandbox


@pytest.fixture(autouse=True)
def select_fleet_for_lifecycle_tests(monkeypatch):
    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: api_key is None))


class FakePool:
    def __init__(self, name: str = "workspace") -> None:
        self.name = name
        self.claims: list[dict] = []
        self.deletes = 0

    async def claim(self, **kwargs):
        self.claims.append(kwargs)
        return SimpleNamespace(name="sandbox-1", claim_name=kwargs.get("name"), pool_name=self.name)

    async def delete(self):
        self.deletes += 1


@pytest.mark.asyncio
async def test_create_with_pool_name_uses_read_only_pool_lookup(monkeypatch):
    pool = FakePool()
    looked_up: list[str] = []

    async def get_pool(cls, name: str):
        looked_up.append(name)
        return pool

    monkeypatch.setattr(Pool, "get", classmethod(get_pool), raising=False)

    sandbox = await Sandbox.create(pool="workspace", name="job-123", service="mcp")

    assert looked_up == ["workspace"]
    assert pool.claims == [
        {"name": "job-123", "spec": None, "service": "mcp", "time_to_start": None}
    ]
    assert sandbox.claim_name == "job-123"


@pytest.mark.asyncio
async def test_create_with_fleet_image_requires_explicit_pool(monkeypatch):
    apply_pool = AsyncMock()
    monkeypatch.setattr(Pool, "apply", apply_pool)

    with pytest.raises(ValueError, match="Pool.apply"):
        await Sandbox.create(
            Image.from_registry("registry.example/workspace:latest"),
            name="job-123",
        )

    apply_pool.assert_not_awaited()


@pytest.mark.asyncio
async def test_pool_create_persists_generated_claim_pool_mapping(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    class ClaimedSandbox:
        name = "bound-sandbox-1"
        claim_name = "generated-claim-1"
        pool_name = "workspace"

    pool = FakePool("workspace")

    async def claim(**kwargs):
        pool.claims.append(kwargs)
        return ClaimedSandbox()

    pool.claim = claim
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    monkeypatch.setattr(Pool, "get", AsyncMock(return_value=pool))

    sandbox = await Sandbox.create(pool="workspace")

    assert sandbox.name == "bound-sandbox-1"
    assert sandbox.claim_name == "generated-claim-1"
    assert sandbox_state.load("generated-claim-1")["pool_name"] == "workspace"


@pytest.mark.parametrize("source", ["existing-pool", "pool-object"])
@pytest.mark.asyncio
async def test_keep_alive_failure_releases_claim_without_persisting_state(
    monkeypatch, tmp_path, source
):
    from cua_sandbox import sandbox_state

    claimed = SimpleNamespace(
        name="bound-sandbox-1",
        claim_name="job-123",
        pool_name="workspace",
        keep_alive=AsyncMock(side_effect=RuntimeError("renew failed")),
        close=AsyncMock(),
    )
    pool = FakePool("workspace")
    pool.claim = AsyncMock(return_value=claimed)
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)

    if source == "existing-pool":
        monkeypatch.setattr(Pool, "get", AsyncMock(return_value=pool))
        create = Sandbox.create(pool="workspace", name="job-123", keep_alive_minutes=30)
    else:
        create = Sandbox.create(pool=pool, name="job-123", keep_alive_minutes=30)

    with pytest.raises(RuntimeError, match="renew failed"):
        await create

    claimed.close.assert_awaited_once()
    assert sandbox_state.load("job-123") is None


@pytest.mark.asyncio
async def test_state_persistence_failure_releases_acquired_claim(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    claimed = SimpleNamespace(
        name="bound-sandbox-1",
        claim_name="job-123",
        pool_name="workspace",
        close=AsyncMock(),
    )
    pool = FakePool("workspace")
    pool.claim = AsyncMock(return_value=claimed)
    monkeypatch.setattr(Pool, "get", AsyncMock(return_value=pool))
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    monkeypatch.setattr(
        sandbox_state,
        "save_fleet_claim",
        lambda *args, **kwargs: (_ for _ in ()).throw(OSError("state write failed")),
    )

    with pytest.raises(OSError, match="state write failed"):
        await Sandbox.create(pool="workspace", name="job-123")

    claimed.close.assert_awaited_once()


@pytest.mark.asyncio
async def test_keep_alive_failure_preserves_error_when_claim_close_fails(monkeypatch):
    claimed = SimpleNamespace(
        name="bound-sandbox-1",
        claim_name="job-123",
        pool_name="workspace",
        keep_alive=AsyncMock(side_effect=RuntimeError("renew failed")),
        close=AsyncMock(side_effect=RuntimeError("close failed")),
    )
    pool = FakePool("workspace")
    pool.claim = AsyncMock(return_value=claimed)

    with pytest.raises(RuntimeError, match="renew failed") as error:
        await Sandbox.create(pool=pool, name="job-123", keep_alive_minutes=30)

    assert isinstance(error.value.__cause__, RuntimeError)
    assert str(error.value.__cause__) == "close failed"


@pytest.mark.asyncio
async def test_create_rejects_pool_and_image_together():
    with pytest.raises(ValueError, match="mutually exclusive"):
        await Sandbox.create(Image.from_registry("registry.example/workspace:latest"), pool="pool")


@pytest.mark.asyncio
async def test_create_rejects_pool_configuration_for_existing_pool():
    with pytest.raises(ValueError, match="existing pool"):
        await Sandbox.create(pool="pool", replicas=2)


class FakeTransport:
    def __init__(self) -> None:
        self.disconnects = 0

    async def connect(self) -> None:
        return None

    async def disconnect(self) -> None:
        self.disconnects += 1


class FakeClaimHandle:
    name = "job-123"
    pool_name = "workspace"

    def __init__(self) -> None:
        self.releases = 0
        self.renewals: list[str] = []

    def to_dict(self):
        return {
            "version": 1,
            "provider": "fleet",
            "namespace": "workspace",
            "pool": "workspace",
            "claim": "job-123",
        }

    async def release(self) -> None:
        self.releases += 1

    async def renew(self, shutdown_time: str) -> None:
        self.renewals.append(shutdown_time)


@pytest.mark.asyncio
async def test_disconnect_keeps_claim_and_close_releases_once():
    transport = FakeTransport()
    handle = FakeClaimHandle()
    sandbox = Sandbox(transport, name="sandbox-1")
    sandbox._claim_handle = handle

    await sandbox.disconnect()
    assert handle.releases == 0

    await sandbox.close()
    await sandbox.close()

    assert handle.releases == 1
    assert transport.disconnects == 2


@pytest.mark.asyncio
async def test_close_removes_persisted_claim_mapping(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    sandbox_state.save_fleet_claim("job-123", "workspace")
    handle = FakeClaimHandle()
    sandbox = Sandbox(FakeTransport(), name="sandbox-1")
    sandbox._claim_handle = handle

    await sandbox.close()

    assert sandbox_state.load("job-123") is None


@pytest.mark.asyncio
async def test_keep_alive_renews_with_utc_deadline():
    handle = FakeClaimHandle()
    sandbox = Sandbox(FakeTransport(), name="sandbox-1")
    sandbox._claim_handle = handle

    await sandbox.keep_alive(minutes=30)

    assert len(handle.renewals) == 1
    assert handle.renewals[0].endswith("Z")


def test_to_dict_serializes_claim_not_bound_transport():
    handle = FakeClaimHandle()
    sandbox = Sandbox(FakeTransport(), name="sandbox-1")
    sandbox._claim_handle = handle

    assert sandbox.claim_name == "job-123"
    assert sandbox.pool_name == "workspace"
    assert sandbox.to_dict()["claim"] == "job-123"
    assert "sandbox-1" not in str(sandbox.to_dict())


@pytest.mark.asyncio
async def test_ephemeral_pool_closes_claim(monkeypatch):
    closed: list[bool] = []

    class Claimed:
        async def close(self):
            closed.append(True)

    async def create(cls, *args, **kwargs):
        return Claimed()

    monkeypatch.setattr(Sandbox, "create", classmethod(create))

    async with Sandbox.ephemeral(pool="workspace"):
        pass

    assert closed == [True]


@pytest.mark.asyncio
async def test_ephemeral_image_uses_name_for_owned_pool_and_claim(monkeypatch):
    claimed = SimpleNamespace(close=AsyncMock())
    pool = FakePool("cua-live-main-source-manual")
    pool.claim = AsyncMock(return_value=claimed)
    applied: list[dict] = []

    async def apply_pool(cls, image, **kwargs):
        applied.append({"image": image, **kwargs})
        return pool

    monkeypatch.setattr(Pool, "apply", classmethod(apply_pool), raising=False)
    image = Image.from_registry("registry.example/workspace:latest")

    async with Sandbox.ephemeral(
        image,
        name="cua-live-main-source-manual",
        cpu=4,
        memory_mb=4096,
    ):
        pass

    assert applied == [
        {
            "image": image,
            "name": "cua-live-main-source-manual",
            "replicas": 1,
            "cpu": 4,
            "memory_mb": 4096,
            "services": {"server": 8000},
        }
    ]
    pool.claim.assert_awaited_once_with(
        name="cua-live-main-source-manual",
        spec=None,
        service="server",
        time_to_start=None,
    )
    claimed.close.assert_awaited_once()
    assert pool.deletes == 1


@pytest.mark.asyncio
async def test_ephemeral_image_keep_pool_reuses_named_pool(monkeypatch):
    claimed = SimpleNamespace(close=AsyncMock())
    pool = FakePool("shared-pool")
    pool.claim = AsyncMock(return_value=claimed)
    apply_pool = AsyncMock(return_value=pool)
    monkeypatch.setattr(Pool, "apply", apply_pool)
    image = Image.from_registry("registry.example/workspace:latest")

    async with Sandbox.ephemeral(image, name="shared-pool", keep_pool=True):
        pass

    assert apply_pool.await_args.kwargs["name"] == "shared-pool"
    claimed.close.assert_awaited_once()
    assert pool.deletes == 0


@pytest.mark.asyncio
async def test_ephemeral_image_keep_pool_requires_name(monkeypatch):
    apply_pool = AsyncMock()
    monkeypatch.setattr(Pool, "apply", apply_pool)

    with pytest.raises(ValueError, match="keep_pool requires name="):
        async with Sandbox.ephemeral(
            Image.from_registry("registry.example/workspace:latest"),
            keep_pool=True,
        ):
            pass

    apply_pool.assert_not_awaited()


@pytest.mark.asyncio
async def test_ephemeral_image_without_name_applies_random_disposable_pool(monkeypatch):
    claimed = SimpleNamespace(close=AsyncMock())
    pool = FakePool("owned-pool")
    pool.claim = AsyncMock(return_value=claimed)
    apply_pool = AsyncMock(return_value=pool)
    monkeypatch.setattr(Pool, "apply", apply_pool)

    async with Sandbox.ephemeral(Image.from_registry("registry.example/workspace:latest")):
        pass

    pool_name = apply_pool.await_args.kwargs["name"]
    assert pool_name.startswith("cua-eph-")
    assert len(pool_name) == len("cua-eph-") + 12
    pool.claim.assert_awaited_once_with(name=None, spec=None, service="server", time_to_start=None)
    claimed.close.assert_awaited_once()
    assert pool.deletes == 1


@pytest.mark.asyncio
async def test_ephemeral_rejects_keep_pool_for_existing_pool():
    with pytest.raises(ValueError, match="keep_pool"):
        async with Sandbox.ephemeral(pool="workspace", keep_pool=True):
            pass


@pytest.mark.asyncio
async def test_ephemeral_rejects_keep_pool_outside_fleet_image_mode():
    with pytest.raises(ValueError, match="keep_pool"):
        async with Sandbox.ephemeral(
            Image.from_registry("registry.example/workspace:latest"),
            api_key="legacy-key",
            keep_pool=True,
        ):
            pass


@pytest.mark.asyncio
async def test_ephemeral_image_deletes_owned_pool_when_claim_fails(monkeypatch):
    pool = FakePool("owned-pool")
    pool.claim = AsyncMock(side_effect=RuntimeError("claim failed"))
    monkeypatch.setattr(Pool, "apply", AsyncMock(return_value=pool))

    with pytest.raises(RuntimeError, match="claim failed"):
        async with Sandbox.ephemeral(Image.from_registry("registry.example/workspace:latest")):
            pass

    assert pool.deletes == 1


@pytest.mark.asyncio
async def test_ephemeral_image_preserves_body_error_when_owned_pool_cleanup_fails(monkeypatch):
    claimed = SimpleNamespace(close=AsyncMock(side_effect=RuntimeError("claim cleanup failed")))
    pool = FakePool("owned-pool")
    pool.claim = AsyncMock(return_value=claimed)
    pool.delete = AsyncMock(side_effect=RuntimeError("pool cleanup failed"))
    monkeypatch.setattr(Pool, "apply", AsyncMock(return_value=pool))

    with pytest.raises(ValueError, match="body failed"):
        async with Sandbox.ephemeral(Image.from_registry("registry.example/workspace:latest")):
            raise ValueError("body failed")

    claimed.close.assert_awaited_once()
    pool.delete.assert_awaited_once()
