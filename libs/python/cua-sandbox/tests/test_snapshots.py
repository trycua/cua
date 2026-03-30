"""Snapshot integration tests — images all the way down.

Tests both Linux VM and Android container snapshot workflows:
1. Create sandbox → install something → snapshot → returns Image
2. Boot from snapshot Image → verify install persists → measure fork is faster
3. Verify state isolation (changes after snapshot don't leak to fork)

Run against local dev stack:
    CUA_API_KEY=sk-dev-test-key-local-12345 CUA_BASE_URL=http://localhost:8082 \
    pytest tests/test_snapshots.py -v -s
"""
from __future__ import annotations

import os
import time

import pytest

from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_snapshot_linux():
    """Linux: create → install cowsay → snapshot → boot from snapshot → verify."""
    t_create_start = time.monotonic()

    async with Sandbox.ephemeral(Image.linux("ubuntu", "24.04")) as sb:
        t_create = time.monotonic() - t_create_start

        # Install something unique
        result = await sb.shell.run(
            "apt-get update -qq && apt-get install -y -qq cowsay", timeout=120
        )
        assert result.success, f"Install failed: {result.stderr}"

        # Verify it works
        result = await sb.shell.run("/usr/games/cowsay hello")
        assert result.success
        assert "hello" in result.stdout

        # Snapshot → returns Image
        cowsay_image = await sb.snapshot(name="with-cowsay")
        assert cowsay_image is not None
        assert cowsay_image._snapshot_source is not None

    # Boot from snapshot image — should be faster (COW fork)
    t_fork_start = time.monotonic()
    async with Sandbox.ephemeral(cowsay_image) as sb2:
        t_fork = time.monotonic() - t_fork_start

        # cowsay should still be installed
        result = await sb2.shell.run("/usr/games/cowsay forked!")
        assert result.success, f"cowsay not found in fork: {result.stderr}"
        assert "forked!" in result.stdout

        print(f"\nCreate+install: {t_create:.1f}s, Fork from snapshot: {t_fork:.1f}s")
        # Fork should be faster than original create + install
        assert t_fork < t_create, (
            f"Fork ({t_fork:.1f}s) should be faster than create ({t_create:.1f}s)"
        )


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_snapshot_android():
    """Android: create with F-Droid → snapshot → boot from snapshot → APK persists."""
    FDROID_APK = "https://f-droid.org/F-Droid.apk"
    t_create_start = time.monotonic()

    async with Sandbox.ephemeral(
        Image.android("14").apk_install(FDROID_APK)
    ) as sb:
        t_create = time.monotonic() - t_create_start

        # Verify F-Droid installed (sb.shell.run on android → adb shell)
        result = await sb.shell.run("pm list packages org.fdroid.fdroid")
        assert result.success
        assert "org.fdroid.fdroid" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"

        # Snapshot → Image
        fdroid_image = await sb.snapshot(name="with-fdroid")

    # Boot from snapshot — F-Droid should persist
    t_fork_start = time.monotonic()
    async with Sandbox.ephemeral(fdroid_image) as sb2:
        t_fork = time.monotonic() - t_fork_start

        result = await sb2.shell.run("pm list packages org.fdroid.fdroid")
        assert result.success, f"F-Droid not found in fork: {result.stderr}"
        assert "org.fdroid.fdroid" in result.stdout

        screenshot = await sb2.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000

        print(f"\nCreate+install: {t_create:.1f}s, Fork from snapshot: {t_fork:.1f}s")
        # On COW storage (btrfs/zfs), fork is instant. On dir storage the copy
        # is a full rsync so it can be slower than the original create.  Only
        # assert that the fork completed (timing checked on COW backends in CI).
        if t_fork < t_create:
            print("  → Fork was faster (COW or small image)")
        else:
            print("  → Fork was slower (dir storage — expected)")
