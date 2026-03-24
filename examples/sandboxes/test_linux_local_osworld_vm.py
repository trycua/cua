"""Run a local Linux VM from a custom qcow2 image with the Cua Sandbox SDK.

    image = Image.from_file("path/to/disk.qcow2", os_type="linux")
    async with Sandbox.ephemeral(image, local=True) as sb:
        screenshot = await sb.screenshot()
        await sb.shell.run("uname -a")

Image.from_file() accepts a local qcow2 path or a remote URL (zip or raw).
The Cua Sandbox SDK downloads, extracts, and caches it automatically.

Pass agent_type="osworld" to use the OSWorld Flask transport instead of
computer-server — useful for running OSWorld benchmarks against existing images.

    image = Image.from_file(OSWORLD_URL, os_type="linux", agent_type="osworld")

Contrast:
    Image.linux()                          -> managed Cua image, auto-provisioned
    Image.from_file("disk.qcow2", ...)     -> your own disk image, local QEMU
    Image.from_file(url, agent_type="osworld") -> OSWorld benchmark image
"""

from __future__ import annotations

import asyncio

import pytest
from cua_sandbox import Image, Sandbox
from cua_sandbox.runtime import QEMURuntime

pytestmark = pytest.mark.asyncio

OSWORLD_IMAGE = (
    "https://huggingface.co/datasets/xlangai/ubuntu_osworld/resolve/main/Ubuntu.qcow2.zip"
)


def _has_osworld_image() -> bool:
    from pathlib import Path

    cache = Path.home() / ".cua" / "cua-sandbox" / "image-cache"
    return any(cache.rglob("Ubuntu.qcow2")) if cache.exists() else False


def _has_qemu() -> bool:
    try:
        from cua_sandbox.runtime.qemu_installer import qemu_bin

        qemu_bin("x86_64")
        return True
    except Exception:
        return False


@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
@pytest.mark.skipif(not _has_osworld_image(), reason="OSWorld Ubuntu.qcow2 not in image cache")
async def test_linux_local_osworld_vm():
    async with Sandbox.ephemeral(
        Image.from_file(OSWORLD_IMAGE, os_type="linux", agent_type="osworld"),
        local=True,
        runtime=QEMURuntime(
            mode="bare-metal", api_port=18020, vnc_display=20, memory_mb=4096, cpu_count=4
        ),
        name="example-linux-local-osworld-vm",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"

        w, h = await sb.get_dimensions()
        assert w == 1920 and h == 1080


async def main():
    async with Sandbox.ephemeral(
        Image.from_file(OSWORLD_IMAGE, os_type="linux", agent_type="osworld"),
        local=True,
        runtime=QEMURuntime(
            mode="bare-metal", api_port=18020, vnc_display=20, memory_mb=4096, cpu_count=4
        ),
        name="example-linux-local-osworld-vm",
    ) as sb:
        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/linux_local_osworld_vm.png", "wb") as f:
            f.write(screenshot)

        w, h = await sb.get_dimensions()
        print(f"Screen: {w}x{h}")


if __name__ == "__main__":
    asyncio.run(main())
