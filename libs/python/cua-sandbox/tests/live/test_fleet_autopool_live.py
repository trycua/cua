"""LIVE: managed Fleet pools end to end (opt-in, creates billed Fleet VMs).

Run only on purpose::

    set -a; source ~/.env; set +a
    CUA_TEST_FLEET_AUTOPOOL_LIVE=1 .venv/bin/python -m pytest -q \\
        tests/live/test_fleet_autopool_live.py -s

What it proves, against real Fleet:

1. ``Sandbox.create(image, local=False)`` twice reuses one managed pool (labelled, KEDA
   autoscaled from zero) and ``Sandbox.list()`` shows both claims.
2. ``Sandbox.ephemeral(image, local=False)`` in a subprocess killed with SIGKILL leaves
   a claim that Fleet reaps within the short test ``claim_ttl``.
3. Cleanup: every claim is released and the test's pool, template and
   namespace are deleted (verified), whatever happens.

The native manager names pools ``cua-auto-*``; the test deletes exactly the
pools its sandboxes landed on, and automatic GC is off, so nothing else is
touched.
"""

from __future__ import annotations

import asyncio
import os
import signal
import subprocess
import sys
import textwrap
import time
from pathlib import Path

import pytest
from cua_sandbox import Image, Sandbox, _autopool

#: Pools this run used (deleted, verified, at the end).
TEST_POOLS: set[str] = set()
CLAIM_TTL = 60
# Pinned image with its own daemon on 8000 (the daemon-agnostic lane).
IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:82702ebdd32d1f8fc05f2ea409a7c67d0ba9f8f8e4e9f1a89ce40989d5f4475d"
)
SERVER_PORT = 8000
BIND_TIMEOUT = 1200
REAP_TIMEOUT = CLAIM_TTL + 180  # TTL + reaper period (30 s) + slack
PACKAGE_ROOT = Path(__file__).resolve().parents[2]

pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(
        os.environ.get("CUA_TEST_FLEET_AUTOPOOL_LIVE") != "1",
        reason="opt-in: set CUA_TEST_FLEET_AUTOPOOL_LIVE=1 (creates billed Fleet VMs)",
    ),
    pytest.mark.skipif(
        not (os.environ.get("CUA_CLIENT_ID") and os.environ.get("CUA_CLIENT_SECRET")),
        reason="Fleet OAuth credentials not set",
    ),
]

CHILD = textwrap.dedent("""
    import asyncio, sys
    from cua_sandbox import Image, Sandbox

    async def main():
        async with Sandbox.ephemeral(
            Image.from_registry(sys.argv[1]),
            server_port=int(sys.argv[2]),
            claim_ttl=int(sys.argv[3]),
            time_to_start=1200,
            telemetry_enabled=False, local=False,
        ) as sb:
            print(f"CLAIM {sb.pool_name} {sb.claim_name}", flush=True)
            for _ in range(3600):  # bounded: killed long before this ends
                await asyncio.sleep(1)

    asyncio.run(main())
    """)


def log(message: str) -> None:
    print(f"[autopool-live {time.strftime('%H:%M:%S')}] {message}", flush=True)


async def claim_names(pool: str) -> set[str]:
    from cua_sandbox.pool import _FleetClient

    client = _FleetClient()
    try:
        return {claim.metadata.name for claim in await client.list_claims(pool)}
    except Exception as error:  # noqa: BLE001
        if _autopool._is_not_found(error):
            return set()
        raise
    finally:
        await client.close()


async def delete_test_pools() -> list[str]:
    """Delete the pools this run used (claims, pool, template, namespace)."""
    from cua_sandbox.pool import _claim_stub, _FleetClient

    client = _FleetClient()
    deleted: list[str] = []
    try:
        for namespace in await client.list_namespaces():
            name = namespace.name
            if name not in TEST_POOLS:
                continue
            for step in ("claims", "pool", "template", "namespace"):
                try:
                    if step == "claims":
                        for claim in await client.list_claims(name):
                            await client.delete_claim(_claim_stub(name, claim.metadata.name))
                    elif step == "pool":
                        await client.delete_pool(await client.get_pool(name))
                    elif step == "template":
                        await client.delete_template(await client.get_template(name, name))
                    else:
                        await client.delete_namespace(name)
                except Exception as error:  # noqa: BLE001
                    if not _autopool._is_not_found(error):
                        log(f"cleanup {step} {name}: {type(error).__name__}: {error}")
            deleted.append(name)
        return deleted
    finally:
        await client.close()


async def remaining_test_namespaces() -> list[str]:
    from cua_sandbox.pool import _FleetClient

    client = _FleetClient()
    try:
        return [ns.name for ns in await client.list_namespaces() if ns.name in TEST_POOLS]
    finally:
        await client.close()


@pytest.fixture
def live_env(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    # Keep the name cache and claim state files off the real ~/.cua.
    monkeypatch.setattr(_autopool, "CUA_DIR", tmp_path / ".cua")
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path / ".cua" / "sandboxes")
    monkeypatch.setenv("CUA_FLEET_POOL_IDLE_GC", "off")
    monkeypatch.setenv("CUA_FLEET_CLAIM_TTL", str(CLAIM_TTL))
    return tmp_path


async def test_managed_pool_reuse_list_and_sigkill_reap(live_env):
    image = Image.from_registry(IMAGE)
    held: list[Sandbox] = []
    child: subprocess.Popen | None = None
    events: list[str] = []
    try:
        # 1. create twice -> one pool
        started = time.monotonic()
        first = await Sandbox.create(
            image,
            server_port=SERVER_PORT,
            time_to_start=BIND_TIMEOUT,
            telemetry_enabled=False,
            progress=lambda event: events.append(f"{event.stage}: {event.message}"),
            local=False,
        )
        held.append(first)
        TEST_POOLS.add(first.pool_name)
        log(
            f"first claim {first.claim_name} on {first.pool_name} in "
            f"{time.monotonic() - started:.0f}s"
        )
        started = time.monotonic()
        second = await Sandbox.create(
            image,
            server_port=SERVER_PORT,
            time_to_start=BIND_TIMEOUT,
            telemetry_enabled=False,
            local=False,
        )
        held.append(second)
        log(
            f"second claim {second.claim_name} on {second.pool_name} in "
            f"{time.monotonic() - started:.0f}s"
        )
        assert first.pool_name == second.pool_name
        assert first.pool_name.startswith("cua-auto-")
        assert any(line.startswith("cold_start") for line in events), events

        [info] = [p for p in await _autopool.list_pools() if p.name == first.pool_name]
        log(f"pool info: {info}")
        assert info.managed, "pool labels (merge patch) did not land"
        assert info.spec_hash, "spec-hash label missing"

        listed = {sb.name: sb for sb in await Sandbox.list()}
        assert first.claim_name in listed and second.claim_name in listed
        assert listed[first.claim_name].pool == first.pool_name

        # 2. ephemeral in a subprocess, SIGKILLed -> claim reaped within TTL
        env = {**os.environ, "PYTHONUNBUFFERED": "1", "HOME": str(live_env)}
        child = subprocess.Popen(
            [sys.executable, "-c", CHILD, IMAGE, str(SERVER_PORT), str(CLAIM_TTL)],
            cwd=PACKAGE_ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        line = ""
        deadline = time.monotonic() + BIND_TIMEOUT + 60
        assert child.stdout is not None
        while time.monotonic() < deadline:  # bounded read of child output
            line = await asyncio.to_thread(child.stdout.readline)
            if not line or line.startswith("CLAIM "):
                break
            log(f"child: {line.rstrip()}")
        assert line.startswith("CLAIM "), f"child exited without a claim: {line!r}"
        _, child_pool, child_claim = line.split()
        assert child_pool == first.pool_name, "ephemeral must reuse the managed pool"
        assert child_claim in {sb.name for sb in await Sandbox.list()}

        os.kill(child.pid, signal.SIGKILL)
        child.wait(timeout=30)
        killed = time.monotonic()
        log(f"SIGKILLed child holding {child_claim}; waiting for Fleet to reap it")
        gone_after = None
        for _ in range(REAP_TIMEOUT // 5 + 1):
            if child_claim not in await claim_names(child_pool):
                gone_after = time.monotonic() - killed
                break
            await asyncio.sleep(5)
        assert gone_after is not None, f"claim {child_claim} outlived {REAP_TIMEOUT}s"
        log(f"claim {child_claim} reaped {gone_after:.0f}s after SIGKILL (ttl {CLAIM_TTL}s)")
        assert child_claim not in {sb.name for sb in await Sandbox.list()}

        # Released claims disappear; the pool stays for reuse.
        for sb in list(held):
            await sb.close()
            held.remove(sb)
        remaining = await claim_names(first.pool_name)
        assert first.claim_name not in remaining and second.claim_name not in remaining
        assert first.pool_name in await remaining_test_namespaces()
    finally:
        if child is not None and child.poll() is None:
            child.kill()
            child.wait(timeout=30)
        for sb in held:
            try:
                await sb.close()
            except Exception as error:  # noqa: BLE001
                log(f"close {sb.claim_name}: {error}")
        _autopool.stop_all_heartbeats()  # no-op: heartbeats live in the SDK
        # 3. cleanup, verified (namespace deletion is asynchronous).
        deleted = await delete_test_pools()
        log(f"deleted test pools {deleted}")
        leftover: list[str] = []
        for _ in range(60):  # up to ~5 min
            leftover = await remaining_test_namespaces()
            if not leftover:
                break
            await asyncio.sleep(5)
        assert not leftover, f"test namespaces left behind: {leftover}"


GVISOR_IMAGE = os.environ.get(
    "CUA_E2E_FLEET_ROOTFS", "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81"
)


async def test_gvisor_managed_pool(live_env):
    """A docker- image defaults to the gVisor runtime; its managed pool says so."""
    from cua_sandbox.pool import _FleetClient

    image = Image.from_registry(GVISOR_IMAGE)
    try:
        started = time.monotonic()
        async with Sandbox.ephemeral(
            image,
            server_port=SERVER_PORT,
            time_to_start=BIND_TIMEOUT,
            telemetry_enabled=False,
            local=False,
        ) as sb:
            log(
                f"gVisor claim {sb.claim_name} on {sb.pool_name} in "
                f"{time.monotonic() - started:.0f}s"
            )
            pool_name, claim_name = sb.pool_name, sb.claim_name
            TEST_POOLS.add(pool_name)
            assert pool_name.startswith("cua-auto-")
            client = _FleetClient()
            try:
                template = await client.get_template(pool_name, pool_name)
            finally:
                await client.close()
            runtime = template.spec.vm_template.runtime
            log(f"template runtime: {runtime}")
            assert str(getattr(runtime, "name", runtime)).lower() == "gvisor"
            [info] = [p for p in await _autopool.list_pools() if p.name == pool_name]
            assert info.managed and info.spec_hash
            response = await sb.services.request("server", method="GET", path="/")
            log(f"server / -> HTTP {response.status_code}")
            assert response.status_code < 500
        assert claim_name not in await claim_names(pool_name)
    finally:
        _autopool.stop_all_heartbeats()  # no-op: heartbeats live in the SDK
        deleted = await delete_test_pools()
        log(f"deleted test pools {deleted}")
        leftover: list[str] = []
        for _ in range(60):  # up to ~5 min
            leftover = await remaining_test_namespaces()
            if not leftover:
                break
            await asyncio.sleep(5)
        assert not leftover, f"test namespaces left behind: {leftover}"
