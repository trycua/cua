"""direct-connect: a spacesd reachable by URL + token and nothing else.

hermetic: the MockServer spacesd from cua-test-fixtures.
container: linux started with plain ``docker run`` (gVisor when
available); the SDK only gets ``connect_url(url, token)``.
"""

from __future__ import annotations

import e2e
import pytest

import cua


async def _direct(c: cua.Cua, url: str, token: str, *, mock: bool) -> dict:
    sbx = c.sandboxes()
    sb = await sbx.connect_url(url, token, e2e.name("direct"))
    try:
        assert sb.location() == "direct"
        env = await e2e.wait_env(sb, 90)
        summary = await e2e.env_smoke(env, desktop=True, mock=mock)

        bad = await sbx.connect_url(url, "wrong-token", e2e.name("direct-bad"))
        with pytest.raises(cua.CuaError.Unauthenticated):
            await bad.spacesd(5000)
        await bad.delete()
        return summary
    finally:
        await sb.delete()


@pytest.mark.e2e("direct-connect", "hermetic")
def test_direct_connect_mock(fixtures, local_cua):
    e2e.run_async(_direct(local_cua, fixtures["env_url"], fixtures["env_token"], mock=True))


@pytest.mark.e2e("direct-connect", "container")
def test_direct_connect_docker(local_cua):
    with e2e.driver_container("direct") as d:
        summary = e2e.run_async(_direct(local_cua, d.url, d.token, mock=False), timeout=300)
        assert summary["os_family"] == "linux"
        assert summary["screen"][0] >= 640, summary
        print("direct-connect summary:", summary, "runtime:", d.runtime)
