"""Run a local Linux container in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.linux(), local=True) as sb:
        await sb.shell.run("uname -a")
        screenshot = await sb.screenshot()

Sandbox.ephemeral() auto-selects DockerRuntime for container images,
provisions the environment, and tears it down on exit. Use local=True to run on
your machine; drop it to run on the Cua cloud instead.

Contrast:
    Image.linux()            + local=True  -> Docker container (fast, no VM overhead)
    Image.linux(kind="vm")   + local=True  -> QEMU VM (full kernel isolation)
    Image.linux()            + local=False -> Cua cloud container
"""

from __future__ import annotations

import asyncio
import subprocess

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio


def _has_docker() -> bool:
    try:
        subprocess.run(["docker", "info"], capture_output=True, check=True, timeout=10)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_linux_local_container():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        local=True,
        name="example-linux-local-container",
    ) as sb:
        result = await sb.shell.run("uname -s")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        local=True,
        name="example-linux-local-container",
    ) as sb:
        result = await sb.shell.run("uname -s")
        print(f"uname: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/linux_local_container.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
