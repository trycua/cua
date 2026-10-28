"""Pools of this machine (``create_pool``): the API reaches ``cua daemon``'s
pool service, and a create from a pool takes its shape and its name."""

from __future__ import annotations

import importlib
from datetime import timedelta
from types import SimpleNamespace

import pytest
from cua_sandbox import (
    Image,
    InvalidArgument,
    Sandbox,
    SandboxPool,
    Unsupported,
    _sdk,
    create_pool,
    delete_pool,
    get_pool,
    list_pools,
)
from cua_sandbox import sync as cua_sync

sandbox_module = importlib.import_module("cua_sandbox.sandbox")
native = pytest.importorskip("cua")._native


class _Sandboxes:
    """``cua daemon``'s pool API, in memory."""

    def __init__(self) -> None:
        self.pools: dict[str, SimpleNamespace] = {}
        self.created: list = []

    async def create_pool(self, image, options):
        self.created.append((image, options))
        if options.on not in (None, "local"):
            raise native.CuaError.Unsupported("pools run on this machine only today")
        name = options.name or "pool-abc123"
        p = SimpleNamespace(
            name=name,
            image=image,
            location="local",
            kind=options.kind or "",
            runtime=options.runtime or "",
            cpus=options.cpus or 2,
            memory_mb=options.memory_mb or 4096,
            min=options.min,
            max=options.max if options.max is not None else options.min,
            idle_ms=options.idle_ms,
            ready=0,
            starting=options.min,
            status=f"starting {options.min}",
        )
        self.pools[name] = p
        return p

    async def get_pool(self, name):
        if name not in self.pools:
            raise native.CuaError.NotFound(f"no pool {name}")
        return self.pools[name]

    async def list_pools(self):
        return list(self.pools.values())

    async def delete_pool(self, name):
        return self.pools.pop(name, None) is not None


@pytest.fixture
def daemon(monkeypatch):
    sbx = _Sandboxes()
    monkeypatch.setattr(_sdk, "daemon_runtime", lambda: SimpleNamespace(sandboxes=lambda: sbx))
    return sbx


async def test_a_pool_is_created_read_listed_and_deleted(daemon):
    pool = await create_pool(
        Image.from_registry("ghcr.io/trycua/linux:24.04-slim", kind="container"),
        name="agents",
        min=2,
        max=8,
        idle=timedelta(minutes=10),
        runtime="gvisor",
        cpu=4,
        memory="8GB",
        env={"A": "1"},
    )
    assert isinstance(pool, SandboxPool)
    assert (pool.name, pool.min, pool.max, pool.idle) == ("agents", 2, 8, timedelta(minutes=10))
    [(image, o)] = daemon.created
    assert image == "ghcr.io/trycua/linux:24.04-slim"
    assert (o.on, o.kind, o.runtime, o.cpus, o.memory_mb) == (
        "local",
        "container",
        "gvisor",
        4,
        8192,
    )
    assert (o.min, o.max, o.idle_ms, o.env) == (2, 8, 600_000, {"A": "1"})
    assert [p.name for p in await list_pools()] == ["agents"]
    assert await get_pool("agents") == pool
    assert await get_pool("nope") is None
    assert await pool.delete() is True
    assert await delete_pool("agents") is False
    assert await list_pools() == []


async def test_min_defaults_to_one_and_bad_sizes_are_refused(daemon):
    pool = await create_pool("python:3.12-slim")
    assert (pool.min, pool.max, pool.idle) == (1, 1, None)
    with pytest.raises(ValueError, match="min"):
        await create_pool("python:3.12-slim", min=-1)
    with pytest.raises(ValueError, match="max"):
        await create_pool("python:3.12-slim", min=3, max=2)
    with pytest.raises(ValueError, match="idle"):
        await create_pool("python:3.12-slim", idle=0)


async def test_a_pool_elsewhere_is_unsupported(daemon):
    with pytest.raises(Unsupported, match="this machine"):
        await create_pool("python:3.12-slim", on="cloud")


def _capture(monkeypatch) -> list:
    seen: list = []

    async def _create(cls, **kwargs):
        seen.append(kwargs)
        return SimpleNamespace(**kwargs)

    monkeypatch.setattr(Sandbox, "_create", classmethod(_create))
    monkeypatch.setattr(sandbox_module, "_with_overlays", _passthrough)
    return seen


async def _passthrough(sb, overlay):
    return sb


async def test_a_create_from_a_pool_takes_its_shape(daemon, monkeypatch):
    seen = _capture(monkeypatch)
    pool = await create_pool("ghcr.io/trycua/linux:24.04-slim", name="agents", cpu=4)
    await pool.create(name="a1", telemetry_enabled=False)
    await Sandbox.create(pool="agents", telemetry_enabled=False)
    for kwargs in seen:
        assert kwargs["local"] is True
        assert kwargs["local_pool"] == "agents"
        assert kwargs["image"]._registry == "ghcr.io/trycua/linux:24.04-slim"
        assert (kwargs["cpu"], kwargs["memory_mb"]) == (4, 4096)
    assert seen[0]["name"] == "a1"


async def test_a_pool_here_cannot_go_to_the_cloud(daemon, monkeypatch):
    _capture(monkeypatch)
    pool = await create_pool("python:3.12-slim", name="agents")
    with pytest.raises(InvalidArgument, match="this machine"):
        await Sandbox.create(pool=pool, on="cloud", telemetry_enabled=False)


async def test_a_name_no_pool_here_has_stays_a_cloud_pool(daemon):
    assert await sandbox_module._sandbox_pool("team", on=None, local=None) is None
    assert await sandbox_module._sandbox_pool("team", on="cloud", local=None) is None


def test_the_blocking_api_wraps_the_same_pools(daemon):
    pool = cua_sync.create_pool("python:3.12-slim", name="agents", min=2)
    assert (pool.name, pool.min) == ("agents", 2)
    assert [p.name for p in cua_sync.list_pools()] == ["agents"]
    assert cua_sync.get_pool("nope") is None
    assert pool.delete() is True
