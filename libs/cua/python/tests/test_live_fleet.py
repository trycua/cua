"""Live Fleet smoke through the Python binding. Gated: set CUA_E2E_FLEET=1
and CUA_CLIENT_ID/CUA_CLIENT_SECRET (or FLEETS_TOKEN). Creates an
ephemeral `cua-e2e-*` pool, claims it, calls the `server` service and
deletes everything in `finally`."""

from __future__ import annotations

import asyncio
import os
import secrets

import pytest

import cua

IMAGE = os.environ.get(
    "CUA_E2E_FLEET_IMAGE",
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81",
)

pytestmark = pytest.mark.skipif(
    os.environ.get("CUA_E2E_FLEET") != "1", reason="set CUA_E2E_FLEET=1 for live Fleet"
)


def test_gvisor_pool_claim_service_status(tmp_path):
    async def body():
        c = cua.embedded(state_dir=str(tmp_path))
        fleet = c.fleet()
        pool = f"cua-e2e-py-{secrets.token_hex(4)}"
        sb = None
        try:
            await fleet.apply_pool(
                cua.FleetPoolSpec(
                    name=pool,
                    image=IMAGE,
                    runtime="gvisor",
                    services={"server": 8000},
                )
            )
            sb = await c.sandboxes().create(
                cua.SandboxCreateOptions(
                    on="cloud",
                    pool=pool,
                    name=f"{pool}-claim",
                    ready_timeout_ms=600_000,
                )
            )
            resp = None
            for _ in range(60):
                resp = await sb.service("server").request("GET", "/status", None, 30_000)
                if resp.status == 200:
                    break
                await asyncio.sleep(5)
            assert resp is not None and resp.status == 200, resp
            print("status body:", resp.body[:200])
        finally:
            if sb is not None:
                await sb.delete()
            try:
                await fleet.delete_pool(pool)
            except cua.CuaError.NotFound:
                pass

    asyncio.run(asyncio.wait_for(body(), timeout=1200))
