"""Run a local Android VM in Python with the Cua Sandbox SDK.

    async with Sandbox.ephemeral(Image.android("14"), local=True) as sb:
        screenshot = await sb.screenshot()
        await sb.mobile.tap(540, 960)

Sandbox.ephemeral() auto-selects AndroidEmulatorRuntime for Android images on a
local host with Android SDK installed. Use local=True to run on your machine;
drop it to run on the Cua cloud instead.

The sb.mobile interface provides touch, gesture, and hardware key controls:
    sb.mobile.tap(x, y)
    sb.mobile.swipe(x1, y1, x2, y2, duration_ms=300)
    sb.mobile.home() / sb.mobile.back() / sb.mobile.recents()
    sb.mobile.type_text("hello")

Contrast:
    Image.android("14")  + local=True  -> Android SDK emulator (this file)
    Image.android("14")  + local=False -> Cua cloud Android VM
"""

from __future__ import annotations

import asyncio

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio


def _has_android_sdk() -> bool:
    try:
        from cua_sandbox.runtime.android_emulator import _find_bin, _sdk_path

        sdk = _sdk_path()
        _find_bin(sdk, "emulator")
        _find_bin(sdk, "adb")
        return True
    except Exception:
        return False


@pytest.mark.skipif(not _has_android_sdk(), reason="Android SDK not available")
async def test_android_local_vm():
    async with Sandbox.ephemeral(
        Image.android("14"),
        local=True,
        name="example-android-local-vm",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 10000

        w, h = await sb.screen.size()
        assert w > 0 and h > 0

        await sb.mobile.home()


async def main():
    async with Sandbox.ephemeral(
        Image.android("14"),
        local=True,
        name="example-android-local-vm",
    ) as sb:
        screenshot = await sb.screenshot()
        print(f"Screenshot: {len(screenshot)} bytes")
        with open("/tmp/android_local_vm.png", "wb") as f:
            f.write(screenshot)

        w, h = await sb.screen.size()
        print(f"Screen size: {w}x{h}")

        await sb.mobile.tap(w // 2, h // 2)
        await sb.mobile.home()
        print("Tap + home: OK")


if __name__ == "__main__":
    asyncio.run(main())
