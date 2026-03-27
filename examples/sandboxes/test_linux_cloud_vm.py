"""Run a cloud Linux VM in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.linux(kind="vm")) as sb:
        await sb.shell.run("uname -a")
        screenshot = await sb.screenshot()

Sandbox.ephemeral() without local=True provisions a full Linux VM on the Cua cloud.
Requires CUA_API_KEY environment variable. No local runtime or Docker needed.

Contrast:
    Image.linux(kind="vm")   + local=False -> Cua cloud VM (this file)
    Image.linux(kind="vm")   + local=True  -> QEMU VM on your machine
    Image.linux()            + local=False -> Cua cloud container (lighter)
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
async def test_linux_cloud_vm():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04", kind="vm"),
        name="example-linux-cloud-vm",
    ) as sb:
        result = await sb.shell.run("uname -s")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04", kind="vm"),
        name="example-linux-cloud-vm",
    ) as sb:
        result = await sb.shell.run("uname -s")
        print(f"uname: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/linux_cloud_vm.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
