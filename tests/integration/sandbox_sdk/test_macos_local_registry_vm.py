"""Run a local macOS VM from a trycua OCI registry image with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/macos-tahoe-cua:latest"),
        local=True,
    ) as sb:
        await sb.shell.run("sw_vers")
        screenshot = await sb.screenshot()

Image.from_registry() fetches the OCI manifest to resolve the image kind and OS,
then auto-selects LumeRuntime for macOS VMs on a macOS host.

Contrast:
    Image.macos("26")                                + local=True  -> Lume VM, managed Cua image (auto-resolved)
    Image.from_registry("ghcr.io/trycua/macos-...") + local=True  -> Lume VM, trycua registry image (manifest-resolved)
    Image.from_registry("ghcr.io/trycua/macos-...") + local=False -> Cua cloud VM
"""

from __future__ import annotations

import asyncio
import platform
import subprocess

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

IS_MACOS = platform.system() == "Darwin"


def _has_lume() -> bool:
    try:
        subprocess.run(["lume", "--version"], capture_output=True, check=True)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


@pytest.mark.skipif(not IS_MACOS or not _has_lume(), reason="Lume only on macOS")
async def test_macos_local_registry_vm():
    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/macos-tahoe-cua:latest"),
        local=True,
        name="example-macos-local-registry-vm",
    ) as sb:
        result = await sb.shell.run("sw_vers")
        assert result.success
        assert "macOS" in result.stdout or "Mac" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/macos-tahoe-cua:latest"),
        local=True,
        name="example-macos-local-registry-vm",
    ) as sb:
        result = await sb.shell.run("sw_vers")
        print(f"sw_vers: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/macos_local_registry_vm.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
