"""Windows Server E2E test using cua-sandbox SDK.

Run:
    CUA_API_KEY=sk-dev-test-key-local-12345 CUA_BASE_URL=http://localhost:8082 \
    uv run pytest tests/test_windows_cloud.py -v -s
"""

import os
import time

import pytest

from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio


def _has_env() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


@pytest.mark.skipif(not _has_env(), reason="CUA_API_KEY not set")
async def test_windows_create_and_snapshot():
    """Image.windows('server-2025') → Sandbox.ephemeral → snapshot → fork."""

    t0 = time.monotonic()

    async with Sandbox.ephemeral(Image.windows("server-2025"), local=False) as sb:
        t_create = time.monotonic() - t0
        print(f"\n  Sandbox ready: {t_create:.1f}s  name={sb.name}")

        # Take a screenshot to prove it works
        screen = await sb.screenshot()
        print(f"  Screenshot: {screen.size if screen else 'None'}")

        # Snapshot
        t1 = time.monotonic()
        snapshot_img = await sb.snapshot()
        t_snap = time.monotonic() - t1
        print(f"  Snapshot: {t_snap:.2f}s")

        # Fork from snapshot
        t2 = time.monotonic()
        async with Sandbox.ephemeral(snapshot_img, local=False) as fork:
            t_fork = time.monotonic() - t2
            print(f"  Fork ready: {t_fork:.1f}s  name={fork.name}")

            fork_screen = await fork.screenshot()
            print(f"  Fork screenshot: {fork_screen.size if fork_screen else 'None'}")

        t_total = time.monotonic() - t0
        print(f"\n  === TIMINGS ===")
        print(f"  Create+ready:  {t_create:.1f}s")
        print(f"  Snapshot:      {t_snap:.2f}s")
        print(f"  Fork+ready:    {t_fork:.1f}s")
        print(f"  Total:         {t_total:.1f}s")
