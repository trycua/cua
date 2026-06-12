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


def P(*a, **kw):
    print(*a, **kw, flush=True)


def _has_env() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


@pytest.mark.skipif(not _has_env(), reason="CUA_API_KEY not set")
async def test_windows_create_and_snapshot():
    """Image.windows('server-2025') -> Sandbox.ephemeral -> snapshot -> fork."""

    t0 = time.monotonic()

    P("\n  Creating Sandbox with Image.windows('server-2025')...")
    async with Sandbox.ephemeral(Image.windows("server-2025"), local=False) as sb:
        t_create = time.monotonic() - t0
        P(f"  Sandbox ready: {t_create:.1f}s  name={sb.name}")

        # Try screenshot
        try:
            screen = await sb.screenshot()
            P(f"  Screenshot: {len(screen)} bytes")
        except Exception as e:
            P(f"  Screenshot not available: {e}")
            screen = None

        # Snapshot
        t1 = time.monotonic()
        P("  Taking snapshot...")
        snapshot_img = await sb.snapshot()
        t_snap = time.monotonic() - t1
        P(f"  Snapshot: {t_snap:.2f}s")

        # Fork from snapshot
        t2 = time.monotonic()
        P("  Forking from snapshot...")
        async with Sandbox.ephemeral(snapshot_img, local=False) as fork:
            t_fork = time.monotonic() - t2
            P(f"  Fork ready: {t_fork:.1f}s  name={fork.name}")

            try:
                fork_screen = await fork.screenshot()
                P(f"  Fork screenshot: {len(fork_screen)} bytes")
            except Exception as e:
                P(f"  Fork screenshot not available: {e}")
                fork_screen = None

        t_total = time.monotonic() - t0
        P("\n  === TIMINGS ===")
        P(f"  Create+ready:  {t_create:.1f}s")
        P(f"  Snapshot:      {t_snap:.2f}s")
        P(f"  Fork+ready:    {t_fork:.1f}s")
        P(f"  Total:         {t_total:.1f}s")


@pytest.mark.skipif(not _has_env(), reason="CUA_API_KEY not set")
async def test_windows_stateful_snapshot_fork():
    """Stateful snapshot captures RAM — fork resumes instantly without rebooting."""

    t0 = time.monotonic()

    P("\n  Creating Sandbox with Image.windows('server-2025')...")
    async with Sandbox.ephemeral(Image.windows("server-2025"), local=False) as sb:
        t_create = time.monotonic() - t0
        P(f"  Sandbox ready: {t_create:.1f}s  name={sb.name}")

        # Verify CUA server is running
        screen = await sb.screenshot()
        P(f"  Screenshot: {len(screen)} bytes")

        # Stateful snapshot — captures memory state
        t1 = time.monotonic()
        P("  Taking stateful snapshot...")
        snapshot_img = await sb.snapshot(stateful=True)
        t_snap = time.monotonic() - t1
        P(f"  Stateful snapshot: {t_snap:.2f}s")

        # Fork from stateful snapshot — should resume instantly
        t2 = time.monotonic()
        P("  Forking from stateful snapshot...")
        async with Sandbox.ephemeral(snapshot_img, local=False) as fork:
            t_fork = time.monotonic() - t2
            P(f"  Fork ready: {t_fork:.1f}s  name={fork.name}")

            # Fork should have CUA server immediately available (no reboot)
            fork_screen = await fork.screenshot()
            P(f"  Fork screenshot: {len(fork_screen)} bytes")
            assert len(fork_screen) > 1000, "Fork screenshot should be non-trivial"

        t_total = time.monotonic() - t0
        P("\n  === STATEFUL TIMINGS ===")
        P(f"  Create+ready:      {t_create:.1f}s")
        P(f"  Stateful snapshot: {t_snap:.2f}s")
        P(f"  Fork+ready:        {t_fork:.1f}s  (should be <10s with stateful)")
        P(f"  Total:             {t_total:.1f}s")
