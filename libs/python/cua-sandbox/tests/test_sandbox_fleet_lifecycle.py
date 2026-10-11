from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from cua_sandbox import Image, Pool, Sandbox, _autopool


@pytest.fixture(autouse=True)
def select_fleet_for_lifecycle_tests(monkeypatch):
    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: api_key is None))


def managed_sandbox(pool_name: str, claim_name: str) -> SimpleNamespace:
    """What _autopool.acquire returns: a sandbox holding a managed claim."""
    return SimpleNamespace(
        name=f"sbx-{claim_name}",
        claim_name=claim_name,
        pool_name=pool_name,
        _claim_handle=SimpleNamespace(
            state_fields=lambda: {"managed": True, "spec_hash": "0" * 64, "claim_ttl": 900}
        ),
        _ephemeral=False,
        telemetry_enabled=False,
        keep_alive=AsyncMock(),
        close=AsyncMock(),
    )


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

    sandbox = await Sandbox.create(pool="workspace", name="job-123", service="mcp", local=False)

    assert looked_up == ["workspace"]
    assert pool.claims == [
        {"name": "job-123", "spec": None, "service": "mcp", "time_to_start": None}
    ]
    assert sandbox.claim_name == "job-123"


@pytest.mark.asyncio
async def test_create_with_fleet_image_claims_from_managed_pool(monkeypatch, tmp_path):
    # Was test_create_with_fleet_image_requires_explicit_pool: a Fleet image
    # without pool= now claims from the account's managed pool.
    from cua_sandbox import sandbox_state

    apply_pool = AsyncMock()
    monkeypatch.setattr(Pool, "apply", apply_pool)
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    acquired = managed_sandbox("cua-auto-abc", "job-123")
    acquire = AsyncMock(return_value=acquired)
    monkeypatch.setattr(_autopool, "acquire", acquire)
    image = Image.from_registry("registry.example/workspace:latest")

    sandbox = await Sandbox.create(image, name="job-123", telemetry_enabled=False, local=False)

    assert sandbox is acquired
    apply_pool.assert_not_awaited()
    assert acquire.await_args.args == (image,)
    assert acquire.await_args.kwargs["name"] == "job-123"
    assert sandbox_state.load("job-123")["pool_name"] == "cua-auto-abc"
    assert sandbox_state.load("job-123")["managed"] is True


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

    sandbox = await Sandbox.create(pool="workspace", local=False)

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
        create = Sandbox.create(
            pool="workspace", name="job-123", keep_alive_minutes=30, local=False
        )
    else:
        create = Sandbox.create(pool=pool, name="job-123", keep_alive_minutes=30, local=False)

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
        await Sandbox.create(pool="workspace", name="job-123", local=False)

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
        await Sandbox.create(pool=pool, name="job-123", keep_alive_minutes=30, local=False)

    assert isinstance(error.value.__cause__, RuntimeError)
    assert str(error.value.__cause__) == "close failed"


@pytest.mark.asyncio
async def test_create_compares_an_image_given_with_a_pool(monkeypatch):
    from cua_sandbox import Pool, PoolSpecMismatch

    seen: list = []

    async def check(name, spec):
        seen.append((name, spec.reference()))
        raise PoolSpecMismatch("image: pool has other@sha256:1, requested ...")

    monkeypatch.setattr(Pool, "check", staticmethod(check))
    with pytest.raises(PoolSpecMismatch, match="image"):
        await Sandbox.create(
            Image.from_registry("registry.example/workspace:latest"), pool="pool", local=False
        )
    assert seen == [("pool", "registry.example/workspace:latest")]


@pytest.mark.asyncio
async def test_create_rejects_pool_configuration_for_existing_pool():
    with pytest.raises(ValueError, match="existing pool"):
        await Sandbox.create(pool="pool", replicas=2, local=False)


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

    async with Sandbox.ephemeral(pool="workspace", local=False):
        pass

    assert closed == [True]


@pytest.mark.asyncio
async def test_ephemeral_image_uses_name_for_the_claim_on_a_managed_pool(monkeypatch):
    # Was test_ephemeral_image_uses_name_for_owned_pool_and_claim: name= now
    # names the claim; the managed pool is reused and never deleted.
    claimed = managed_sandbox("cua-auto-abc", "cua-live-main-source-manual")
    acquire = AsyncMock(return_value=claimed)
    monkeypatch.setattr(_autopool, "acquire", acquire)
    apply_pool = AsyncMock()
    monkeypatch.setattr(Pool, "apply", apply_pool)
    image = Image.from_registry("registry.example/workspace:latest")

    async with Sandbox.ephemeral(
        image,
        name="cua-live-main-source-manual",
        cpu=4,
        memory_mb=4096,
        telemetry_enabled=False,
        local=False,
    ):
        pass

    apply_pool.assert_not_awaited()
    kwargs = acquire.await_args.kwargs
    assert kwargs["name"] == "cua-live-main-source-manual"
    assert (kwargs["cpu"], kwargs["memory_mb"], kwargs["service"]) == (4, 4096, "env")
    claimed.close.assert_awaited_once()


@pytest.mark.parametrize(
    "kwargs",
    [
        {"image": "fleet", "name": "shared-pool"},
        {"image": "fleet"},
        {"pool": "workspace"},
        {"image": "fleet", "api_key": "legacy-key"},
    ],
    ids=["named", "unnamed", "existing-pool", "legacy-cloud"],
)
@pytest.mark.asyncio
async def test_keep_pool_is_a_deprecated_no_op(monkeypatch, kwargs):
    # Replaces the keep_pool tests (reuses named pool / requires name= /
    # rejected for existing pools / rejected outside Fleet image mode):
    # pools are reused automatically, so keep_pool only warns.
    claimed = managed_sandbox("cua-auto-abc", "claim-1")
    monkeypatch.setattr(_autopool, "acquire", AsyncMock(return_value=claimed))
    monkeypatch.setattr(Sandbox, "create", AsyncMock(return_value=claimed))
    legacy = SimpleNamespace(_has_snapshots=False, name="legacy", destroy=AsyncMock())
    monkeypatch.setattr(Sandbox, "_create", AsyncMock(return_value=legacy))
    if kwargs.get("image") == "fleet":
        kwargs = {**kwargs, "image": Image.from_registry("registry.example/workspace:latest")}

    with pytest.warns(DeprecationWarning, match="keep_pool"):
        async with Sandbox.ephemeral(
            keep_pool=True, telemetry_enabled=False, **kwargs, local=False
        ):
            pass


@pytest.mark.asyncio
async def test_ephemeral_image_without_name_never_creates_a_disposable_pool(monkeypatch):
    # Was test_ephemeral_image_without_name_applies_random_disposable_pool:
    # no cua-eph-* pool is applied or deleted any more.
    claimed = managed_sandbox("cua-auto-abc", "generated-claim")
    acquire = AsyncMock(return_value=claimed)
    monkeypatch.setattr(_autopool, "acquire", acquire)
    apply_pool = AsyncMock()
    monkeypatch.setattr(Pool, "apply", apply_pool)

    async with Sandbox.ephemeral(
        Image.from_registry("registry.example/workspace:latest"),
        telemetry_enabled=False,
        local=False,
    ):
        pass

    apply_pool.assert_not_awaited()
    assert acquire.await_args.kwargs["name"] is None
    claimed.close.assert_awaited_once()


@pytest.mark.asyncio
async def test_ephemeral_image_propagates_acquire_failure(monkeypatch):
    # Was test_ephemeral_image_deletes_owned_pool_when_claim_fails: the
    # manager releases its own claim; there is no owned pool to delete.
    monkeypatch.setattr(_autopool, "acquire", AsyncMock(side_effect=RuntimeError("claim failed")))

    with pytest.raises(RuntimeError, match="claim failed"):
        async with Sandbox.ephemeral(
            Image.from_registry("registry.example/workspace:latest"), local=False
        ):
            pass


@pytest.mark.asyncio
async def test_ephemeral_image_preserves_body_error_when_claim_cleanup_fails(monkeypatch):
    # Was ..._when_owned_pool_cleanup_fails.
    claimed = managed_sandbox("cua-auto-abc", "claim-1")
    claimed.close = AsyncMock(side_effect=RuntimeError("claim cleanup failed"))
    monkeypatch.setattr(_autopool, "acquire", AsyncMock(return_value=claimed))

    with pytest.raises(ValueError, match="body failed"):
        async with Sandbox.ephemeral(
            Image.from_registry("registry.example/workspace:latest"),
            telemetry_enabled=False,
            local=False,
        ):
            raise ValueError("body failed")

    claimed.close.assert_awaited_once()


@pytest.mark.asyncio
async def test_ephemeral_forwards_the_fleet_runtime_to_the_managed_pool(monkeypatch):
    # Was ..._to_the_owned_pool: the runtime is part of the managed pool key.
    claimed = managed_sandbox("cua-auto-abc", "claim-1")
    acquire = AsyncMock(return_value=claimed)
    monkeypatch.setattr(_autopool, "acquire", acquire)
    image = Image.from_registry("public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-1")

    async with Sandbox.ephemeral(image, runtime="gvisor", telemetry_enabled=False, local=False):
        pass

    assert acquire.await_args.kwargs["runtime"] == "gvisor"


@pytest.mark.asyncio
async def test_a_fleet_runtime_belongs_to_the_pool_not_the_claim(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    monkeypatch.setattr(Pool, "get", AsyncMock(side_effect=AssertionError("no lookup")))
    with pytest.raises(ValueError, match="Pool.apply"):
        await Sandbox.create(pool="workspace", runtime="gvisor", local=False)
    # Without pool=, the runtime selects the managed pool.
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    acquire = AsyncMock(return_value=managed_sandbox("cua-auto-abc", "job"))
    monkeypatch.setattr(_autopool, "acquire", acquire)
    await Sandbox.create(
        Image.from_registry("registry.example/workspace:latest"),
        name="job",
        runtime="gvisor",
        telemetry_enabled=False,
        local=False,
    )
    assert acquire.await_args.kwargs["runtime"] == "gvisor"
