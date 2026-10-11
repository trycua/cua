"""The Fleet guides, through the cua SDK (``cua``) directly.

* your-first-cloud-fleet: apply pool -> claim -> ``uname -a`` -> screenshot
  PNG -> delete.
* create-fleet-capacity / claim-a-sandbox (formerly create-pool-with-*): warm pool, readiness, named claim
  reattach, replica scaling, an ephemeral sandbox releases its claim on a
  managed ``cua-auto-*`` pool (the test then GCs that pool), name collision
  is a typed error.
* expire-pools-and-claims: pool TTL and claim TTL reach Fleet.

hermetic: the fake Fleet API (+ the MockServer spacesd for the desktop
steps). fleet: live run.cua.ai with the guides' public legacy image (its
desktop is computer-server on 8000, reached through the ``server``
service). fleet-env: live Fleet with the spacesd reference image.
"""

from __future__ import annotations

import base64
import json
import os
import re
import secrets

import e2e
import pytest

import cua

FAKE_IMAGE = "registry.test/cua-e2e:fake"


def _is_404(e: Exception) -> bool:
    # A Fleet 404, and the 403 live Fleet answers for reads once a pool's
    # namespace is gone, are CuaError.NotFound (test_fleet_404_is_not_found);
    # a delete in a namespace that is already gone still answers 403.
    return isinstance(e, cua.CuaError.NotFound) or (
        isinstance(e, cua.CuaError.Fleet) and "403" in str(e)
    )


async def _cleanup(fleet: cua.Fleet, pool: str) -> None:
    try:
        await fleet.delete_pool(pool)
    except cua.CuaError as e:
        if not _is_404(e):
            raise


async def _assert_gone(fleet: cua.Fleet, pool: str, attempts: int = 90) -> None:
    """Fleet deletes asynchronously (finalizers): poll until the pool is gone."""

    async def gone():
        try:
            await fleet.get_pool(pool)
        except cua.CuaError as e:
            if _is_404(e):
                return True
            raise
        return False

    await e2e.poll(f"pool {pool} deleted", gone, attempts=attempts, delay=2, retry=())


async def _claim_gone(fleet: cua.Fleet, pool: str, claim: str) -> None:
    async def gone():
        return claim not in [cl.name for cl in await fleet.list_claims(pool)]

    await e2e.poll(f"claim {claim} released", gone, attempts=90, delay=2, retry=())


@pytest.mark.e2e("create-pool", "hermetic")
def test_fleet_404_is_not_found(fake_fleet):
    async def body():
        with pytest.raises(cua.CuaError.NotFound):
            await fake_fleet.fleet().get_pool(e2e.name("missing"))

    e2e.run_async(body(), timeout=60)


def _pool_of(sb: cua.Sandbox) -> str:
    """The pool (namespace) of a Fleet sandbox, from its gateway URL."""
    for url in sb.info().endpoints.values():
        m = re.search(r"/api/svc/([^/]+)/", url)
        if m:
            return m.group(1)
    raise AssertionError(f"no gateway endpoint in {sb.info()}")


# ------------------------------------------------------------ your-first-cloud-fleet


async def _first_fleet(
    c: cua.Cua,
    *,
    image: str,
    runtime: str,
    desktop,
    services: dict,
    command: list[str] | None = None,
    token: str | None = None,
    what: str = "first",
) -> dict:
    fleet = c.fleet()
    # Distinct per test: a just-deleted pool's namespace stays forbidden (403)
    # while Fleet finalizes it.
    pool = e2e.name(what)
    sb = None
    try:
        await fleet.apply_pool(
            cua.FleetPoolSpec(
                name=pool,
                image=image,
                runtime=runtime,
                replicas=1,
                cpu=4,
                memory_mb=4096,
                services=services,
                ttl_seconds_after_created=7200,
                command=command,
            )
        )
        sb = await c.sandboxes().create(
            cua.SandboxCreateOptions(
                on="cloud",
                pool=pool,
                name=f"{pool}-claim",
                token=token,
                ready_timeout_ms=1_200_000,
            )
        )
        assert sb.location() == "cloud"
        uname, png = await desktop(sb)
        assert "Linux" in uname, uname
        assert e2e.is_png(png), png[:16]
        result = {"uname": uname.strip(), "png_size": e2e.png_size(png)}
    finally:
        if sb is not None:
            await sb.delete()
        await _cleanup(fleet, pool)
    await _assert_gone(fleet, pool)
    return result


async def _legacy_desktop(sb: cua.Sandbox):
    """computer-server's /cmd through the authenticated `server` service."""
    await e2e.poll(
        "computer-server /status",
        lambda: _ok(sb.service("server").request("GET", "/status", None, 30_000)),
        attempts=90,
        delay=5,
    )
    out = await e2e.legacy_cmd(sb, "server", "run_command", {"command": "uname -a"})
    shot = await e2e.legacy_cmd(sb, "server", "screenshot")
    data = shot.get("image_data") or shot.get("result", {}).get("image_data")
    return out.get("stdout", ""), base64.b64decode(data)


async def _ok(fut):
    r = await fut
    return r if r.status == 200 else None


@pytest.mark.e2e("your-first-cloud-fleet", "hermetic")
def test_first_cloud_fleet_fake(fake_fleet, fixtures):
    async def desktop(sb):
        # The fake gateway cannot host a spacesd; the desktop steps run
        # against the MockServer spacesd by URL.
        assert (await sb.service("server").request("GET", "/status", None, 5_000)).status == 200
        d = await fake_fleet.sandboxes().connect_url(
            fixtures["env_url"], fixtures["env_token"], e2e.name("first-env")
        )
        try:
            env = await d.spacesd(5_000)
            out = await env.run(cua.SpacesdCommand(program="echo", args=["Linux", "mock"]))
            return out.stdout.decode(), (await env.screenshot(None)).image
        finally:
            await d.delete()

    print(
        e2e.run_async(
            _first_fleet(
                fake_fleet,
                image=FAKE_IMAGE,
                runtime="kubevirt",
                desktop=desktop,
                services={"server": 8000},
            ),
            timeout=120,
        )
    )


@pytest.mark.e2e("your-first-cloud-fleet", "fleet")
def test_first_cloud_fleet_live(live_fleet):
    """Exactly the tutorial's pool: the pinned containerDisk on KubeVirt."""
    print(
        e2e.run_async(
            _first_fleet(
                live_fleet,
                image=e2e.LEGACY_FLEET_IMAGE,
                runtime="kubevirt",
                desktop=_legacy_desktop,
                services={"server": 8000},
            ),
            timeout=1800,
        )
    )


@pytest.mark.e2e("your-first-cloud-fleet", "fleet-env")
def test_first_cloud_fleet_env_image(live_fleet):
    """The tutorial's flow on the spacesd image, as a gVisor pool (the env
    token rides in through an entrypoint override)."""
    token = secrets.token_hex(16)

    async def desktop(sb):
        env = await e2e.wait_env(sb, 180)
        out = await env.sh("uname -a", None)
        return out.stdout.decode(), (await env.screenshot(None)).image

    print(
        e2e.run_async(
            _first_fleet(
                live_fleet,
                image=os.environ["CUA_E2E_FLEET_ENV_IMAGE"],
                runtime="gvisor",
                desktop=desktop,
                services={"env": 3211},
                command=e2e.env_token_command(token),
                token=token,
                what="first-env",
            ),
            timeout=1800,
        )
    )


@pytest.mark.e2e("your-first-cloud-fleet", "fleet-env")
def test_first_cloud_fleet_env_image_kubevirt(live_fleet):
    pytest.skip(e2e.KUBEVIRT_ENV_SKIP)


# ------------------------------------------------------------ create-pool


async def _create_pool(c: cua.Cua, *, image: str, runtime: str, live: bool) -> None:
    fleet = c.fleet()
    pool = e2e.name("pool")
    claim = f"{pool}-claim"
    try:
        applied = await fleet.apply_pool(
            cua.FleetPoolSpec(
                name=pool,
                image=image,
                runtime=runtime,
                replicas=1,
                cpu=1,
                memory_mb=2048,
                services={"server": 8000},
                ttl_seconds_after_created=7200,
            )
        )
        assert applied.name == pool and applied.replicas == 1
        # Pool.apply reconciles: applying again is idempotent.
        again = await fleet.apply_pool(
            cua.FleetPoolSpec(
                name=pool,
                image=image,
                runtime=runtime,
                replicas=1,
                cpu=1,
                memory_mb=2048,
                services={"server": 8000},
                ttl_seconds_after_created=7200,
            )
        )
        assert again.name == pool
        if live:
            ready = await fleet.wait_pool_ready(pool, 900_000)
            assert (ready.ready_replicas or 0) >= 1, ready

        sbx = c.sandboxes()
        sb = await sbx.create(
            cua.SandboxCreateOptions(on="cloud", pool=pool, name=claim, ready_timeout_ms=900_000)
        )
        # A named claim reattaches instead of creating a second one.
        sb2 = await sbx.create(
            cua.SandboxCreateOptions(on="cloud", pool=pool, name=claim, ready_timeout_ms=900_000)
        )
        claims = [cl.name for cl in await fleet.list_claims(pool)]
        assert claims.count(claim) == 1, claims
        assert sb2.name() == sb.name()
        resp = await e2e.poll(
            "server service",
            lambda: _ok(sb2.service("server").request("GET", "/status", None, 30_000)),
            attempts=60 if live else 3,
            delay=5 if live else 0.1,
        )
        assert resp.status == 200

        scaled = await fleet.set_pool_replicas(pool, 2)
        assert scaled.replicas == 2
        assert (await fleet.get_pool(pool)).replicas == 2

        # Exiting the claim releases it; the pool stays warm.
        await sb.delete()
        await _claim_gone(fleet, pool, claim)
        assert (await fleet.get_pool(pool)).name == pool
    finally:
        await _cleanup(fleet, pool)


async def _ephemeral(c: cua.Cua, *, image: str, runtime: str) -> None:
    fleet = c.fleet()
    sb = await c.sandboxes().create(
        cua.SandboxCreateOptions(
            on="cloud",
            image=image,
            runtime=runtime,
            services={"server": 8000},
            cpus=1,
            memory_mb=2048,
            fleet_ttl_seconds=3600,
            ready_timeout_ms=900_000,
        )
    )
    pool = _pool_of(sb)
    try:
        try:
            assert sb.is_ephemeral()
            # Managed pools are named cua-auto-<tenant/spec hash> and outlive
            # the sandbox (reused by the next create with the same spec).
            assert pool.startswith("cua-auto-"), pool
            assert (await fleet.get_pool(pool)).name == pool
        finally:
            await sb.delete()
        await _claims_released(fleet, pool)
        assert (await fleet.get_pool(pool)).name == pool, "the managed pool stays for reuse"
    finally:
        await _gc_managed_pool(fleet, pool)
    await _assert_gone(fleet, pool)


async def _claims_released(fleet: cua.Fleet, pool: str) -> None:
    async def none_left():
        return not await fleet.list_claims(pool)

    await e2e.poll(f"claims of {pool} released", none_left, attempts=90, delay=2, retry=())


async def _gc_managed_pool(fleet: cua.Fleet, pool: str) -> None:
    """Delete the managed pool this test used, through the SDK's scoped GC
    (only this pool, only once it has no claims), so runs leave nothing."""
    report = await fleet.pools().gc_pools([pool], 0)
    assert not report.errors, report.errors


@pytest.mark.e2e("create-pool", "hermetic")
def test_create_pool_fake(fake_fleet):
    e2e.run_async(
        _create_pool(fake_fleet, image=FAKE_IMAGE, runtime="gvisor", live=False), timeout=120
    )
    e2e.run_async(_ephemeral(fake_fleet, image=FAKE_IMAGE, runtime="gvisor"), timeout=120)


@pytest.mark.e2e("create-pool", "fleet")
def test_create_pool_live(live_fleet):
    e2e.run_async(
        _create_pool(live_fleet, image=e2e.LEGACY_FLEET_ROOTFS, runtime="gvisor", live=True),
        timeout=1800,
    )


@pytest.mark.e2e("create-pool", "fleet")
def test_ephemeral_pool_cleaned_up_live(live_fleet):
    e2e.run_async(
        _ephemeral(live_fleet, image=e2e.LEGACY_FLEET_ROOTFS, runtime="gvisor"), timeout=1800
    )


@pytest.mark.e2e("create-pool", "fleet")
def test_pool_name_collision_is_typed(live_fleet):
    """Pool names are global; a name another account owns is a typed error.
    Needs a name known to be taken (CUA_E2E_FLEET_TAKEN_POOL): the suite
    never guesses one, so it never creates a pool in someone else's space."""
    taken = os.environ.get("CUA_E2E_FLEET_TAKEN_POOL")
    if not taken:
        pytest.skip("set CUA_E2E_FLEET_TAKEN_POOL to a pool name owned by another account")

    async def body():
        with pytest.raises((cua.CuaError.PermissionDenied, cua.CuaError.Fleet)) as err:
            await live_fleet.fleet().apply_pool(
                cua.FleetPoolSpec(name=taken, image=e2e.LEGACY_FLEET_ROOTFS, runtime="gvisor")
            )
        assert "403" in str(err.value) or "denied" in str(err.value).lower(), err.value

    e2e.run_async(body(), timeout=120)


# ------------------------------------------------------------ expire-pools-and-claims


def _has_ttl(obj_json: str, seconds: int) -> bool:
    obj = json.loads(obj_json)
    found = []

    def walk(o):
        if isinstance(o, dict):
            for k, v in o.items():
                if k in ("ttlSecondsAfterCreated", "ttl_seconds_after_created") and v == seconds:
                    found.append(k)
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    walk(obj)
    return bool(found)


async def _expire(c: cua.Cua, *, image: str, live: bool) -> None:
    fleet = c.fleet()
    pool = e2e.name("ttl")
    try:
        await fleet.apply_pool(
            cua.FleetPoolSpec(
                name=pool,
                image=image,
                runtime="gvisor",
                replicas=0 if live else 1,
                cpu=1,
                memory_mb=1024,
                services={"server": 8000},
                ttl_seconds_after_created=86400,
            )
        )
        got = await fleet.get_pool(pool)
        assert _has_ttl(got.json, 86400), got.json[:600]
        # Re-applying does not reset the age (the TTL is still the spec's).
        await fleet.apply_pool(
            cua.FleetPoolSpec(
                name=pool,
                image=image,
                runtime="gvisor",
                replicas=0 if live else 1,
                cpu=1,
                memory_mb=1024,
                services={"server": 8000},
                ttl_seconds_after_created=86400,
            )
        )
        claim = await fleet.claim(pool, f"{pool}-claim", 3600)
        claims = {cl.name: cl for cl in await fleet.list_claims(pool)}
        body = claims[claim.name].json
        assert _has_ttl(body, 3600) or "shutdownTime" in body, body[:600]
        if "shutdownTime" in body:
            assert "Delete" in body, body[:600]
        await fleet.release(pool, claim.name)
    finally:
        await _cleanup(fleet, pool)


@pytest.mark.e2e("expire-pools-and-claims", "hermetic")
def test_expire_fake(fake_fleet):
    e2e.run_async(_expire(fake_fleet, image=FAKE_IMAGE, live=False), timeout=120)


@pytest.mark.e2e("expire-pools-and-claims", "fleet")
def test_expire_live(live_fleet):
    e2e.run_async(_expire(live_fleet, image=e2e.LEGACY_FLEET_ROOTFS, live=True), timeout=900)
