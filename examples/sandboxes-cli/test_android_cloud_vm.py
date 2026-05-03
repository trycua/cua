"""Launch a persistent cloud Android VM via the CLI.

    cua sb launch android:14 --json
    # parse name from JSON output, connect with SDK, run assertions
    cua sb delete <name>

Mirrors examples/sandboxes/test_android_cloud_vm.py but exercises the CLI
launch path and persistent state tracking instead of Sandbox.ephemeral().
Requires CUA_API_KEY. No Android SDK or emulator needed.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess

import pytest
from cua_sandbox import Sandbox

pytestmark = pytest.mark.asyncio


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


def _cua(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["cua", *args], capture_output=True, text=True)


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_android_cloud_vm():
    result = _cua("sb", "launch", "android:14", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]

    try:
        async with Sandbox.connect(name) as sb:
            screenshot = await sb.screenshot()
            assert screenshot[:4] == b"\x89PNG"
            assert len(screenshot) > 1000

            w, h = await sb.screen.size()
            assert w > 0 and h > 0

            await sb.mobile.home()
    finally:
        _cua("sb", "delete", name)


async def main():
    r = _cua("sb", "launch", "android:14", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        async with Sandbox.connect(name) as sb:
            screenshot = await sb.screenshot()
            print(f"Screenshot: {len(screenshot)} bytes")
            with open("/tmp/cli_android_cloud_vm.png", "wb") as f:
                f.write(screenshot)
            w, h = await sb.screen.size()
            print(f"Screen size: {w}x{h}")
            await sb.mobile.tap(w // 2, h // 2)
            await sb.mobile.home()
            print("Tap + home: OK")
    finally:
        _cua("sb", "delete", name)


if __name__ == "__main__":
    asyncio.run(main())
