"""Run a local Linux VM from a trycua OCI registry image with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/cua-xfce:latest"),
        local=True,
        runtime=TartRuntime(),
    ) as sb:
        await sb.shell.run("uname -a")
        screenshot = await sb.screenshot()

TartRuntime uses Apple's Virtualization.framework (via Tart CLI) to run OCI VM images.
Image.from_registry() is required — TartRuntime only works with OCI registry references.

Contrast:
    Image.linux("ubuntu", "24.04", kind="vm") + local=True              -> QEMU VM, managed Cua image
    Image.from_registry("ghcr.io/trycua/...") + local=True + TartRuntime -> Tart VM, OCI registry image (macOS only)
    Image.from_registry("ghcr.io/trycua/...") + local=False              -> Cua cloud VM
"""

from __future__ import annotations

import asyncio
import platform
import subprocess

import pytest
from cua_sandbox import Image, Sandbox
from cua_sandbox.runtime import TartRuntime

pytestmark = pytest.mark.asyncio

IS_MACOS = platform.system() == "Darwin"


def _has_tart() -> bool:
    try:
        subprocess.run(["tart", "--version"], capture_output=True, check=True)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


@pytest.mark.skipif(not IS_MACOS or not _has_tart(), reason="Tart only on macOS")
async def test_linux_local_registry_vm():
    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/cua-xfce:latest"),
        local=True,
        runtime=TartRuntime(),
        name="example-linux-local-registry-vm",
    ) as sb:
        result = await sb.shell.run("uname -s")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/cua-xfce:latest"),
        local=True,
        runtime=TartRuntime(),
        name="example-linux-local-registry-vm",
    ) as sb:
        result = await sb.shell.run("uname -s")
        print(f"uname: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/linux_local_registry_vm.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
