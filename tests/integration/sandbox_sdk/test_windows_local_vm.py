"""Run a local Windows VM in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.windows("11"), local=True) as sb:
        await sb.shell.run("ver")
        screenshot = await sb.screenshot()

Sandbox.ephemeral() auto-selects HyperVRuntime on Windows hosts with Hyper-V,
or QEMURuntime (bare-metal) as a fallback. Use local=True to run on your machine;
drop it to run on the Cua cloud instead.

Contrast:
    Image.windows("11")  + local=True  -> Hyper-V or QEMU VM (this file)
    Image.windows("11")  + local=False -> Cua cloud Windows VM
"""

from __future__ import annotations

import asyncio
import platform

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

IS_WINDOWS = platform.system() == "Windows"


def _has_qemu() -> bool:
    try:
        from cua_sandbox.runtime.qemu_installer import qemu_bin

        qemu_bin("x86_64")
        return True
    except Exception:
        return False


@pytest.mark.skipif(not IS_WINDOWS, reason="Windows host only")
@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
async def test_windows_local_vm():
    async with Sandbox.ephemeral(
        Image.windows("11"),
        local=True,
        name="example-windows-local-vm",
    ) as sb:
        result = await sb.shell.run("ver")
        assert result.success or "Windows" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


async def main():
    async with Sandbox.ephemeral(
        Image.windows("11"),
        local=True,
        name="example-windows-local-vm",
    ) as sb:
        result = await sb.shell.run("ver")
        print(f"ver: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/windows_local_vm.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
