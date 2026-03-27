"""Run a cloud Android VM in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.android("14")) as sb:
        screenshot = await sb.screenshot()
        await sb.mobile.tap(540, 960)

Sandbox.ephemeral() without local=True provisions an Android VM on the Cua cloud.
Requires CUA_API_KEY environment variable. No Android SDK or emulator needed.

The sb.mobile interface provides touch, gesture, and hardware key controls:
    sb.mobile.tap(x, y)
    sb.mobile.swipe(x1, y1, x2, y2, duration_ms=300)
    sb.mobile.home() / sb.mobile.back() / sb.mobile.recents()
    sb.mobile.type_text("hello")

Contrast:
    Image.android("14")  + local=False -> Cua cloud Android VM (this file)
    Image.android("14")  + local=True  -> Android SDK emulator on your machine
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
async def test_android_cloud_vm():
    async with Sandbox.ephemeral(
        Image.android("14"),
        name="example-android-cloud-vm",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000

        w, h = await sb.screen.size()
        assert w > 0 and h > 0

        await sb.mobile.home()


async def main():
    async with Sandbox.ephemeral(
        Image.android("14"),
        name="example-android-cloud-vm",
    ) as sb:
        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/android_cloud_vm.png", "wb") as f:
            f.write(screenshot)

        w, h = await sb.screen.size()
        print(f"Screen size: {w}x{h}")

        await sb.mobile.tap(w // 2, h // 2)
        await sb.mobile.home()
        print("Tap + home: OK")


if __name__ == "__main__":
    asyncio.run(main())
