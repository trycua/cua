"""Run a cloud macOS VM in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.macos("26")) as sb:
        await sb.shell.run("sw_vers")
        screenshot = await sb.screenshot()

Sandbox.ephemeral() without local=True provisions a macOS VM on the Cua cloud.
Requires CUA_API_KEY environment variable. Works on any host OS — no Mac needed.

Contrast:
    Image.macos("26")  + local=False -> Cua cloud macOS VM (this file)
    Image.macos("26")  + local=True  -> Lume VM, macOS host only
"""

from __future__ import annotations

import asyncio
import os

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_macos_cloud_vm():
    async with Sandbox.ephemeral(
        Image.macos("26"),
        name="example-macos-cloud-vm",
    ) as sb:
        result = await sb.shell.run("sw_vers")
        assert result.success
        assert "macOS" in result.stdout or "Mac" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.macos("26"),
        name="example-macos-cloud-vm",
    ) as sb:
        result = await sb.shell.run("sw_vers")
        print(f"sw_vers: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/macos_cloud_vm.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
