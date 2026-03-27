"""Run a cloud Windows VM in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.windows("11")) as sb:
        await sb.shell.run("ver")
        screenshot = await sb.screenshot()

Sandbox.ephemeral() without local=True provisions a Windows VM on the Cua cloud.
Requires CUA_API_KEY environment variable. Works on any host OS — no Windows needed.

Contrast:
    Image.windows("11")  + local=False -> Cua cloud Windows VM (this file)
    Image.windows("11")  + local=True  -> Hyper-V or QEMU VM on your machine
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
async def test_windows_cloud_vm():
    async with Sandbox.ephemeral(
        Image.windows("11"),
        name="example-windows-cloud-vm",
    ) as sb:
        result = await sb.shell.run("ver")
        assert result.success or "Windows" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.windows("11"),
        name="example-windows-cloud-vm",
    ) as sb:
        result = await sb.shell.run("ver")
        print(f"ver: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/windows_cloud_vm.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
