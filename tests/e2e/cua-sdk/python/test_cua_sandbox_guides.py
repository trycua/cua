"""cua-sandbox lane: the guides' own public API (``cua_sandbox.Pool``,
``Image``, ``Sandbox.ephemeral``) on top of the cua SDK.

Cua Cloud has ended (CUA-1854): every ``Pool`` call raises
``cua_sandbox._ended.CloudEnded``, so each check asserts that instead of a
Fleet round trip. Gated by CUA_E2E_CUA_SANDBOX=1 because cua-sandbox is being
rewritten as a thin wrapper over ``cua``; enable the lane after that merge.
Each check runs in the docs venv (repo cua-sandbox) as a subprocess.
"""

from __future__ import annotations

import subprocess
import textwrap

import e2e
import pytest
from test_docs_blocks import _env, sandbox_python  # noqa: F401  (session fixture)


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


def _expect_cloud_ended(py: str, call: str) -> None:
    """Runs ``call`` (an awaitable expression on ``Pool``) in the docs venv
    and checks it raises ``CloudEnded`` with the one message."""
    _run(
        py,
        f"""
        import asyncio
        from cua_sandbox import Image, Pool
        from cua_sandbox._ended import CloudEnded
        async def main():
            try:
                await {call}
            except CloudEnded as e:
                assert "Cua Cloud has ended" in str(e), e
                return
            raise SystemExit("expected CloudEnded")
        asyncio.run(main())
    """,
        {},
    )


@pytest.mark.e2e("expire-pools-and-claims", "cua-sandbox")
def test_pool_apply_with_a_ttl_is_cloud_ended(sandbox_python):
    pool = e2e.name("cs-ttl")
    _expect_cloud_ended(
        sandbox_python,
        f"Pool.apply(Image.from_registry({e2e.LEGACY_FLEET_ROOTFS!r}), name={pool!r}, "
        "replicas=1, cpu=1, memory_mb=1024, ttl_seconds_after_created=3600)",
    )


@pytest.mark.e2e("create-pool", "cua-sandbox")
def test_pool_apply_with_autoscaling_is_cloud_ended(sandbox_python):
    pool = e2e.name("cs-auto")
    _expect_cloud_ended(
        sandbox_python,
        f"Pool.apply(Image.from_registry({e2e.LEGACY_FLEET_ROOTFS!r}), name={pool!r}, cpu=1, "
        "memory_mb=2048, autoscaling=None, ttl_seconds_after_created=3600)",
    )


@pytest.mark.e2e("create-pool", "cua-sandbox")
def test_pool_get_is_cloud_ended(sandbox_python):
    _expect_cloud_ended(sandbox_python, f"Pool.get({e2e.name('cs-get')!r})")
