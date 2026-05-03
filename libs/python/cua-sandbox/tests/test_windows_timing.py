"""Compare Windows VM restore timings: fresh create vs snapshot vs stateful snapshot.

Run:
    CUA_API_KEY=sk-dev-test-key-local-12345 CUA_BASE_URL=http://localhost:8082 \
    uv run pytest tests/test_windows_timing.py -v -s
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
async def test_windows_all_restore_modes():
    """Compare: Image.windows() vs snapshot() vs snapshot(stateful=True) fork times."""

    results = {}

    # ── 1. Fresh create from Image.windows("server-2025") ──
    P("\n  [1/3] Creating fresh VM from Image.windows('server-2025')...")
    t0 = time.monotonic()
    async with Sandbox.ephemeral(Image.windows("server-2025"), local=False) as sb:
        t_create = time.monotonic() - t0
        P(f"        Ready in {t_create:.1f}s  name={sb.name}")
        results["create"] = t_create

        screen = await sb.screenshot()
        P(f"        Screenshot: {len(screen)} bytes")

        # ── 2. Non-stateful snapshot + fork ──
        P("\n  [2/3] Taking non-stateful snapshot...")
        t1 = time.monotonic()
        snap_img = await sb.snapshot(stateful=False)
        t_snap = time.monotonic() - t1
        P(f"        Snapshot: {t_snap:.2f}s")
        results["snapshot"] = t_snap

        P("        Forking from non-stateful snapshot...")
        t2 = time.monotonic()
        async with Sandbox.ephemeral(snap_img, local=False) as fork1:
            t_fork1 = time.monotonic() - t2
            P(f"        Fork ready: {t_fork1:.1f}s  name={fork1.name}")
            results["fork_nonstateful"] = t_fork1

            fork1_screen = await fork1.screenshot()
            P(f"        Screenshot: {len(fork1_screen)} bytes")

        # ── 3. Stateful snapshot + fork ──
        P("\n  [3/3] Taking stateful snapshot...")
        t3 = time.monotonic()
        snap_stateful = await sb.snapshot(stateful=True)
        t_snap_s = time.monotonic() - t3
        P(f"        Stateful snapshot: {t_snap_s:.2f}s")
        results["snapshot_stateful"] = t_snap_s

        P("        Forking from stateful snapshot...")
        t4 = time.monotonic()
        async with Sandbox.ephemeral(snap_stateful, local=False) as fork2:
            t_fork2 = time.monotonic() - t4
            P(f"        Fork ready: {t_fork2:.1f}s  name={fork2.name}")
            results["fork_stateful"] = t_fork2

            fork2_screen = await fork2.screenshot()
            P(f"        Screenshot: {len(fork2_screen)} bytes")

    P(f"\n  {'='*50}")
    P("  TIMING COMPARISON")
    P(f"  {'='*50}")
    P(f"  Image.windows() create:     {results['create']:6.1f}s")
    P(f"  snapshot(stateful=False):    {results['snapshot']:6.2f}s")
    P(f"  fork from non-stateful:     {results['fork_nonstateful']:6.1f}s")
    P(f"  snapshot(stateful=True):     {results['snapshot_stateful']:6.2f}s")
    P(f"  fork from stateful:         {results['fork_stateful']:6.1f}s")
    P(f"  {'='*50}")
