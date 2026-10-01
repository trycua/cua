"""Managed Fleet pools (cua_sandbox._autopool) on the cua SDK's native pool
manager, hermetically.

``cua-test-fixtures`` (``cargo build -p cua-daemon --features test-fixtures
--bin cua-test-fixtures``) serves a fake Fleet API on loopback; both the
native manager and cua-sandbox's own Fleet client talk to it. ``~/.cua`` and
the sandbox state directory are redirected to a temp dir. No network, host,
VM or Fleet resources are touched. Without the fixture binary the native
tests skip.
"""

from __future__ import annotations

import importlib
from datetime import datetime, timedelta, timezone

import pytest
from cua_sandbox import Image, Sandbox, _autopool, _sdk, sandbox_state
from cua_sandbox import sync as cua_sync

sandbox_module = importlib.import_module("cua_sandbox.sandbox")

#: Digest-pinned: the native manager never asks a registry to resolve it.
IMAGE = "registry.example/workspace@sha256:0123"


def image() -> Image:
    return Image.from_registry(IMAGE)


async def managed_pools(client_factory) -> list[str]:
    client = client_factory()
    try:
        return sorted(
            n.name for n in await client.list_namespaces() if n.name.startswith("cua-auto-")
        )
    finally:
        await client.close()


async def pool_of(client_factory, name: str):
    client = client_factory()
    try:
        return await client.get_pool(name)
    finally:
        await client.close()


async def claims_in(client_factory, pool: str) -> dict:
    client = client_factory()
    try:
        return {c.metadata.name: c for c in await client.list_claims(pool)}
    finally:
        await client.close()


# ── config ───────────────────────────────────────────────────────────────


def test_config_defaults_env_and_kwargs():
    cfg = _autopool.config(environ={})
    assert (cfg.max_pool_size, cfg.claim_ttl, cfg.warm, cfg.initial_pool_size) == (
        10,
        900,
        False,
        0,
    )
    # The idle GC default matches the Rust SDK and the docs: 30 min.
    assert cfg.idle_gc == 30 * 60
    cfg = _autopool.config(
        environ={
            "CUA_FLEET_MAX_POOL_SIZE": "4",
            "CUA_FLEET_CLAIM_TTL": "2m",
            "CUA_FLEET_WARM": "1",
            "CUA_FLEET_POOL_IDLE_GC": "1h",
        }
    )
    assert (cfg.max_pool_size, cfg.claim_ttl, cfg.initial_pool_size, cfg.idle_gc) == (
        4,
        120,
        1,
        3600,
    )
    cfg = _autopool.config(
        environ={"CUA_FLEET_WARM": "1"}, warm=False, claim_ttl=timedelta(minutes=1)
    )
    assert (cfg.warm, cfg.claim_ttl) == (False, 60)
    assert not _autopool.config(environ={"CUA_FLEET_POOL_IDLE_GC": "off"}).auto_gc
    assert _autopool.config(environ={}).bind_deadline == 900


@pytest.mark.parametrize(
    "kwargs, message",
    [
        ({"max_pool_size": 0}, "max_pool_size"),
        ({"claim_ttl": 5}, "claim_ttl must be between"),
        ({"claim_ttl": "10m"}, "claim_ttl must be seconds"),
    ],
)
def test_config_rejects_bad_values(kwargs, message):
    with pytest.raises((ValueError, TypeError), match=message):
        _autopool.config(environ={}, **kwargs)


def test_config_rejects_bad_env_duration():
    with pytest.raises(ValueError, match="CUA_FLEET_CLAIM_TTL"):
        _autopool.config(environ={"CUA_FLEET_CLAIM_TTL": "soon"})


# ── spec key and naming (contract helpers) ──────────────────────────────


def test_spec_key_is_stable_and_covers_the_spec():
    base = _autopool.spec_key(image())
    assert base.spec_hash() == _autopool.spec_key(image()).spec_hash()
    assert base.services == (("env", 3211),)
    variants = {
        _autopool.spec_key(image(), cpu=4).spec_hash(),
        _autopool.spec_key(image(), memory_mb=8192).spec_hash(),
        _autopool.spec_key(image(), server_port=8000).spec_hash(),
        _autopool.spec_key(Image.from_registry("registry.example/other:1")).spec_hash(),
        _autopool.spec_key(image().expose(5900)).spec_hash(),
    }
    assert base.spec_hash() not in variants
    assert len(variants) == 5
    assert len(base.label) == 32


def test_pool_names_are_dns_labels_scoped_by_tenant():
    spec_hash = _autopool.spec_key(image()).spec_hash()
    first = _autopool.pool_name_for("tenant-a", spec_hash)
    assert first == _autopool.pool_name_for("tenant-a", spec_hash)
    assert first != _autopool.pool_name_for("tenant-b", spec_hash)
    assert first.startswith("cua-auto-") and len(first) == len("cua-auto-") + 16
    assert first == first.lower() and first.replace("-", "").isalnum()
    assert _autopool.candidate_names(first)[1:3] == [f"{first}-2", f"{first}-3"]
    assert len(_autopool.candidate_names(first)) == 8


def test_canonical_encoding_matches_the_rust_pool_manager():
    # Golden vector for cua_fleet::autopool::PoolSpecKey::new("img").canonical()
    # and auto_pool_name("tenant-a", hash): both SDKs must name a spec alike.
    key = _autopool.PoolSpecKey(image="img")
    assert key.canonical() == (
        b'["cua-autopool/v1",["image","img"],["runtime","kubevirt"],["efi",false],'
        b'["cpu",null],["memory_mb",null],["services",[["env",3211]]],'
        b'["readiness_tcp_port",null],["command",null]]'
    )
    import base64
    import hashlib

    digest = hashlib.sha256(b"tenant-a\0" + key.spec_hash().encode()).digest()
    expected = "cua-auto-" + base64.b32encode(digest).decode().lower()[:16]
    assert _autopool.pool_name_for("tenant-a", key.spec_hash()) == expected


def test_managed_key_matches_sandbox_core_defaults():
    key = _autopool.spec_key(image().expose(5900), server_port=8000)
    assert (key.cpu, key.memory_mb) == (2, 4096)
    assert dict(key.services) == {"env": 3211, "server": 8000, "port-5900": 5900}
    assert key.readiness_tcp_port == 8000


# ── native manager: acquire / reuse ──────────────────────────────────────


async def test_create_without_pool_claims_from_a_managed_pool(fleet):
    sb = await Sandbox.create(image(), local=False)

    [pool_name] = await managed_pools(fleet)
    assert sb.pool_name == pool_name
    pool = await pool_of(fleet, pool_name)
    assert pool.spec.replicas == 0
    assert pool.spec.autoscaling.min_pool_size == 0
    assert pool.spec.autoscaling.initial_pool_size == 0
    assert pool.spec.autoscaling.max_pool_size == 10
    assert pool.spec.ttl_seconds_after_created == 7 * 86400
    labels = pool.metadata.labels
    assert labels["cua.ai/managed-by"] == "cua-sdk"
    assert len(labels["cua.ai/spec-hash"]) == 32

    claim = (await claims_in(fleet, pool_name))[sb.claim_name]
    assert claim.spec.ttl_seconds_after_created == 900
    assert claim.spec.bind_deadline >= 900
    assert claim.metadata.labels["cua.ai/managed-by"] == "cua-sdk"
    assert sb._claim_handle.heartbeat_running

    state = sandbox_state.load(sb.claim_name)
    assert state["runtime_type"] == "fleet"
    assert state["pool_name"] == pool_name
    assert state["managed"] is True
    assert state["claim_ttl"] == 900
    # The native manager's pool-name cache lives in CUA_DIR (0600).
    cache = _autopool.CUA_DIR / "fleet-pools.json"
    assert pool_name in cache.read_text()
    assert cache.stat().st_mode & 0o777 == 0o600

    await sb.close()
    assert not sb._claim_handle.heartbeat_running
    assert sb.claim_name not in await claims_in(fleet, pool_name)
    assert await managed_pools(fleet) == [pool_name], "the pool stays for reuse"
    assert sandbox_state.load(sb.claim_name) is None


async def test_same_spec_reuses_and_different_specs_do_not(fleet):
    first = await Sandbox.create(image(), local=False)
    second = await Sandbox.create(image(), local=False)
    assert first.pool_name == second.pool_name
    assert first.claim_name != second.claim_name
    other = await Sandbox.create(image(), cpu=4, local=False)
    assert other.pool_name != first.pool_name
    assert len(await managed_pools(fleet)) == 2
    for sb in (first, second, other):
        await sb.close()


async def test_warm_max_pool_size_and_claim_ttl(fleet):
    sb = await Sandbox.create(image(), warm=True, max_pool_size=3, claim_ttl=120, local=False)
    pool = await pool_of(fleet, sb.pool_name)
    assert pool.spec.replicas == 1
    assert pool.spec.autoscaling.initial_pool_size == 1
    assert pool.spec.autoscaling.max_pool_size == 3
    claim = (await claims_in(fleet, sb.pool_name))[sb.claim_name]
    assert claim.spec.ttl_seconds_after_created == 120
    await sb.close()


async def test_long_time_to_start_raises_the_bind_deadline(fleet):
    sb = await Sandbox.create(image(), time_to_start=1800, local=False)
    claim = (await claims_in(fleet, sb.pool_name))[sb.claim_name]
    assert claim.spec.bind_deadline == 1800
    await sb.close()


async def test_named_create_reattaches_an_existing_claim(fleet):
    first = await Sandbox.create(image(), name="job-1", local=False)
    await first.disconnect()
    second = await Sandbox.create(image(), name="job-1", local=False)
    assert second.claim_name == "job-1"
    assert list(await claims_in(fleet, second.pool_name)) == ["job-1"]
    await second.close()


async def test_keep_alive_extends_a_managed_claim(fleet):
    sb = await Sandbox.create(image(), keep_alive_minutes=90, local=False)
    claim = (await claims_in(fleet, sb.pool_name))[sb.claim_name]
    deadline = datetime.fromisoformat(claim.spec.lifecycle.shutdown_time.replace("Z", "+00:00"))
    assert deadline - datetime.now(timezone.utc) > timedelta(minutes=89)
    await sb.close()


async def test_server_port_publishes_and_waits_for_the_server_service(fleet):
    sb = await Sandbox.create(image(), server_port=8000, local=False)
    assert sb._claim_handle.service == "server"
    client = fleet()
    try:
        template = await client.get_template(sb.pool_name, sb.pool_name)
    finally:
        await client.close()
    services = {s.name: s.target_port for s in template.spec.vm_template.services}
    assert services == {"env": 3211, "server": 8000}
    await sb.close()


async def test_osworld_image_claims_the_flask_server_through_the_managed_pool(fleet):
    """Legacy OSWorld adapter on Fleet: an ``agent_type="osworld"`` disk has no
    spacesd, so the managed pool publishes its Flask server as ``server``
    on 5000, readiness waits on it, and the claim speaks the OSWorld API."""
    from cua_sandbox.transport.fleet import OSWorldFleetTransport

    osworld = Image.from_registry(IMAGE, os_type="linux", kind="vm", agent_type="osworld")
    sb = await Sandbox.create(osworld, local=False)
    assert sb._claim_handle.service == "server"
    assert sb._claim_handle.agent_type == "osworld"
    assert isinstance(sb._transport, OSWorldFleetTransport)
    client = fleet()
    try:
        template = await client.get_template(sb.pool_name, sb.pool_name)
    finally:
        await client.close()
    services = {s.name: s.target_port for s in template.spec.vm_template.services}
    assert services == {"env": 3211, "server": 5000}
    assert sandbox_state.load(sb.claim_name)["agent_type"] == "osworld"
    # A plain image of the same reference is a different managed pool.
    plain = await Sandbox.create(image(), local=False)
    assert plain.pool_name != sb.pool_name
    await plain.close()
    await sb.close()


async def test_disconnect_detaches_and_keeps_the_claim(fleet):
    sb = await Sandbox.create(image(), local=False)
    await sb.disconnect()
    assert not sb._claim_handle.heartbeat_running
    assert sb.claim_name in await claims_in(fleet, sb.pool_name)


# ── ephemeral ────────────────────────────────────────────────────────────


async def test_ephemeral_releases_the_claim_and_keeps_the_pool(fleet):
    async with Sandbox.ephemeral(image(), local=False) as sb:
        pool_name, claim_name = sb.pool_name, sb.claim_name
        assert claim_name in await claims_in(fleet, pool_name)
        assert sandbox_state.load(claim_name) is None
    assert claim_name not in await claims_in(fleet, pool_name)
    assert await managed_pools(fleet) == [pool_name]
    async with Sandbox.ephemeral(image(), local=False) as again:
        assert again.pool_name == pool_name
    assert not await claims_in(fleet, pool_name)


async def test_ephemeral_releases_on_body_error(fleet):
    with pytest.raises(RuntimeError, match="body"):
        async with Sandbox.ephemeral(image(), local=False) as sb:
            raise RuntimeError("body")
    assert not await claims_in(fleet, sb.pool_name)


async def test_keep_pool_is_a_deprecated_no_op(fleet):
    with pytest.warns(DeprecationWarning, match="keep_pool"):
        async with Sandbox.ephemeral(image(), keep_pool=True, local=False) as sb:
            pass
    assert await managed_pools(fleet) == [sb.pool_name]


# ── list / pools / gc ────────────────────────────────────────────────────


async def test_list_pools_and_gc_through_the_native_manager(fleet):
    sb = await Sandbox.create(image(), local=False)
    [info] = await _autopool.list_pools()
    assert info.name == sb.pool_name
    assert info.managed and info.claims == 1
    assert info.last_used is not None
    assert {i.name for i in await Sandbox.list(local=False)} >= {sb.claim_name}
    rows = {i.name: i for i in await Sandbox.list()}
    assert rows[sb.claim_name].location == "cloud"
    kept = await _autopool.gc(idle_after=3600)
    assert kept.pools_deleted == []
    await sb.close()
    report = await _autopool.gc(idle_after=0)
    assert report.pools_deleted == [sb.pool_name]
    assert await managed_pools(fleet) == []


@pytest.mark.parametrize("operation", ["suspend", "restart"])
async def test_lifecycle_ops_are_unsupported_on_managed_pools(fleet, operation):
    sb = await Sandbox.create(image(), local=False)
    with pytest.raises(NotImplementedError, match="cannot suspend a single sandbox"):
        await getattr(Sandbox, operation)(sb.claim_name)
    await sb.close()


async def test_concurrent_first_use_in_one_process_creates_one_pool(fleet):
    import asyncio

    sandboxes = await asyncio.gather(*(Sandbox.create(image(), local=False) for _ in range(6)))
    assert len({sb.pool_name for sb in sandboxes}) == 1
    assert len(await managed_pools(fleet)) == 1
    for sb in sandboxes:
        await sb.close()


# ── sync facade ──────────────────────────────────────────────────────────


def test_sync_facade_create_ephemeral_and_list(fleet):
    sb = cua_sync.Sandbox.create(image(), claim_ttl=60, local=False)
    assert sb._claim_handle.heartbeat_running
    assert sb.claim_name in [info.name for info in cua_sync.Sandbox.list(local=False)]
    pool = sb.pool_name
    sb.close()
    with cua_sync.Sandbox.ephemeral(image(), local=False) as eph:
        assert eph.pool_name == pool


def test_native_runtime_is_shared_per_pool_home(fleet):
    a = _autopool._native_cua()
    assert a is _autopool._native_cua()
    assert _sdk.runtime(fleet=True) is not a


async def test_resume_of_a_live_cloud_sandbox_reconnects(fleet):
    sb = await Sandbox.create(image(), local=False)
    again = await Sandbox.resume(sb.claim_name)
    assert again.name == sb.claim_name
    await again.disconnect()
    await sb.close()


# -- the default listing and the session marker (a stand-in keychain) --------


async def test_default_list_reads_a_keychain_session_only_with_the_marker(
    fleet, monkeypatch, tmp_path
):
    import datetime
    import json
    import os

    sb = await Sandbox.create(image(), local=False)
    token = os.environ["FLEETS_TOKEN"]
    home, vault = tmp_path / "cua-home", tmp_path / "vault"
    vault.mkdir()
    expires = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(hours=1)
    (vault / "vault.json").write_text(
        json.dumps({"access_token": token, "expires_at": expires.isoformat()})
    )
    # Only the signed-in session (in the stand-in vault) can reach Fleet now.
    monkeypatch.delenv("FLEETS_TOKEN")
    monkeypatch.delenv("CUA_FLEET_SESSION", raising=False)
    monkeypatch.setenv("CUA_HOME", str(home))
    monkeypatch.setenv("CUA_CREDENTIAL_STORE", f"test-keychain:{vault}")

    def reads() -> int:
        try:
            return int((vault / "reads").read_text())
        except FileNotFoundError:
            return 0

    # No marker: the vault is never probed, no cloud rows.
    names = [i.name for i in await Sandbox.list()]
    assert sb.claim_name not in names
    assert reads() == 0

    # With the marker the default listing reads the session and lists the
    # cloud sandbox.
    home.mkdir(exist_ok=True)
    (home / "session.json").write_text(
        json.dumps({"store": "keychain", "account": None, "expires_at": expires.isoformat()})
    )
    rows = {i.name: i for i in await Sandbox.list()}
    assert rows[sb.claim_name].location == "cloud"
    assert reads() > 0
    monkeypatch.setenv("FLEETS_TOKEN", token)
    await sb.close()
