"""Run a cloud Linux container in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.linux()) as sb:
        await sb.shell.run("uname -a")
        screenshot = await sb.screenshot()

Sandbox.ephemeral() without local=True provisions a container on the Cua cloud.
Requires CUA_API_KEY environment variable. No Docker or local runtime needed.

Contrast:
    Image.linux()            + local=False -> Cua cloud container (this file)
    Image.linux()            + local=True  -> Docker container on your machine
    Image.linux(kind="vm")   + local=False -> Cua cloud VM
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
async def test_linux_cloud_container():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        name="example-linux-cloud-container",
    ) as sb:
        result = await sb.shell.run("uname -s")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        name="example-linux-cloud-container",
    ) as sb:
        result = await sb.shell.run("uname -s")
        print(f"uname: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/linux_cloud_container.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
