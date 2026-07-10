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
import logging
import os
import time

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger("test_linux_cloud_vm")


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_linux_cloud_vm():
    t0 = time.monotonic()
    logger.info("Creating ephemeral Linux cloud VM (ubuntu 24.04, kind=vm)")

    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04", kind="vm"),
    ) as sb:
        provision_time = time.monotonic() - t0
        logger.info(
            "VM ready: name=%s, provisioned in %.1fs",
            getattr(sb, "name", "unknown"),
            provision_time,
        )

        # Log transport/endpoint info if available
        transport = getattr(sb, "_transport", None)
        if transport:
            base_url = getattr(getattr(transport, "_inner", None), "_base_url", None)
            logger.info("Transport base_url=%s", base_url)

        t1 = time.monotonic()
        result = await sb.shell.run("uname -s")
        logger.info(
            "shell.run('uname -s'): success=%s, stdout=%r, took=%.1fs",
            result.success,
            result.stdout.strip(),
            time.monotonic() - t1,
        )
        assert result.success, f"uname failed: stderr={result.stderr}"
        assert "Linux" in result.stdout, f"Expected 'Linux' in stdout, got: {result.stdout!r}"

        t2 = time.monotonic()
        screenshot = await sb.screenshot()
        logger.info(
            "screenshot: %d bytes, took=%.1fs",
            len(screenshot),
            time.monotonic() - t2,
        )
        assert screenshot[:4] == b"\x89PNG", f"Screenshot not PNG: first 4 bytes = {screenshot[:4]!r}"

    total_time = time.monotonic() - t0
    logger.info("Test passed in %.1fs (provision=%.1fs)", total_time, provision_time)


async def main():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04", kind="vm"),
    ) as sb:
        result = await sb.shell.run("uname -s")
        print(f"uname: {result.stdout.strip()}")

        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/linux_cloud_vm.png", "wb") as f:
            f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
