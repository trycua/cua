"""cua-sandbox lane: the guides' own public API (``cua_sandbox.Pool``,
``Image``, ``ClaimSpec``, ``Sandbox.ephemeral``) on top of the cua SDK.

Gated by CUA_E2E_CUA_SANDBOX=1 because cua-sandbox is being rewritten as a
thin wrapper over ``cua``; enable the lane after that merge. Each check runs
in the docs venv (repo cua-sandbox) as a subprocess. Fleet: the fake Fleet
API by default (hermetic), live Fleet when CUA_E2E_FLEET=1.
"""

from __future__ import annotations

import os
import subprocess
import textwrap

import e2e
import pytest
from test_docs_blocks import _env, sandbox_python  # noqa: F401  (session fixture)


def _fleet_env(fixtures) -> dict:
    if os.environ.get("CUA_E2E_FLEET") == "1":
        return {}
    return {
        "CUA_FLEET_BASE_URL": fixtures["fleet_base_url"],
        "FLEETS_TOKEN": fixtures["fleet_token"],
        "CUA_CLIENT_ID": "",
        "CUA_CLIENT_SECRET": "",
    }


def _run(py: str, code: str, env: dict, timeout: int = 1800) -> str:
    out = subprocess.run(
        [py, "-c", textwrap.dedent(code)],
        capture_output=True,
        text=True,
        timeout=timeout,
        env=_env(py, env),
    )
    assert out.returncode == 0, out.stderr[-3000:]
    return out.stdout


@pytest.mark.e2e("expire-pools-and-claims", "cua-sandbox")
def test_spec_and_ttl_together_is_value_error(sandbox_python, fixtures):
    pool = e2e.name("cs-ttl")
    _run(
        sandbox_python,
        f"""
        import asyncio
        from cua_sandbox import ClaimSpec, Image, Pool
        async def main():
            pool = await Pool.apply(Image.from_registry({e2e.LEGACY_FLEET_ROOTFS!r}), name={pool!r},
                                    replicas=1, cpu=1, memory_mb=1024, ttl_seconds_after_created=3600)
            try:
                spec = ClaimSpec(sandbox_template_ref=pool.resource.spec.sandbox_template_ref, warmpool=None,
                                 bind_deadline=None, lifecycle=None, ttl_seconds_after_created=3600)
                try:
                    async with pool.claim(spec=spec, ttl_seconds_after_created=60):
                        raise SystemExit("claim with spec + ttl must raise ValueError")
                except ValueError as e:
                    print("ValueError:", e)
            finally:
                await pool.delete()
        asyncio.run(main())
    """,
        _fleet_env(fixtures),
    )


@pytest.mark.e2e("create-pool", "cua-sandbox")
def test_autoscaling_and_ephemeral(sandbox_python, fixtures):
    pool = e2e.name("cs-auto")
    _run(
        sandbox_python,
        f"""
        import asyncio
        from cua_sandbox import Image, Pool, Sandbox, WarmPoolAutoscaling
        async def main():
            pool = await Pool.apply(Image.from_registry({e2e.LEGACY_FLEET_ROOTFS!r}), name={pool!r}, cpu=1, memory_mb=2048,
                                    autoscaling=WarmPoolAutoscaling(min_pool_size=0, initial_pool_size=0, max_pool_size=2),
                                    ttl_seconds_after_created=3600)
            await pool.delete()
        asyncio.run(main())
    """,
        _fleet_env(fixtures),
    )


@pytest.mark.e2e("create-pool", "cua-sandbox")
def test_pool_access_denied_on_taken_name(sandbox_python):
    taken = os.environ.get("CUA_E2E_FLEET_TAKEN_POOL")
    if os.environ.get("CUA_E2E_FLEET") != "1" or not taken:
        pytest.skip(
            "needs live Fleet and CUA_E2E_FLEET_TAKEN_POOL (a pool name another account owns)"
        )
    _run(
        sandbox_python,
        f"""
        import asyncio
        from cua_sandbox import Image, Pool, PoolAccessDeniedError
        async def main():
            try:
                await Pool.apply(Image.from_registry({e2e.LEGACY_FLEET_ROOTFS!r}), name={taken!r})
            except PoolAccessDeniedError as e:
                assert "globally unique" in str(e), e
                return
            raise SystemExit("expected PoolAccessDeniedError")
        asyncio.run(main())
    """,
        {},
    )
