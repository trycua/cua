"""cua-sandbox lane: the public API (``Image``, ``Sandbox``) on top of the cua
SDK, as the guides use it.

Cua Cloud has ended, so each check asserts that asking for it raises
``cua_sandbox._ended.CloudEnded`` with the one message. Gated by
CUA_E2E_CUA_SANDBOX=1. Each check runs in the docs venv (repo cua-sandbox) as a
subprocess.
"""

from __future__ import annotations

import subprocess
import textwrap

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
    """Runs ``call`` (an awaitable expression on ``Sandbox``) in the docs venv
    and checks it raises ``CloudEnded`` with the one message."""
    _run(
        py,
        f"""
        import asyncio
        from cua_sandbox import Image, Sandbox
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


@pytest.mark.e2e("cloud-ended", "cua-sandbox")
def test_on_cloud_is_cloud_ended(sandbox_python):
    _expect_cloud_ended(sandbox_python, 'Sandbox.create(Image.linux(), on="cloud")')


@pytest.mark.e2e("cloud-ended", "cua-sandbox")
def test_local_false_is_cloud_ended(sandbox_python):
    _expect_cloud_ended(sandbox_python, "Sandbox.create(Image.linux(), local=False)")


@pytest.mark.e2e("cloud-ended", "cua-sandbox")
def test_a_pool_name_is_cloud_ended(sandbox_python):
    _expect_cloud_ended(sandbox_python, 'Sandbox.create(Image.linux(), pool="my-pool")')
