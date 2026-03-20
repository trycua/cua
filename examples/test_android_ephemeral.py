"""Test ephemeral Android cloud sandbox end-to-end."""

import asyncio
import time

from cua_sandbox import Image, Sandbox


async def main():
    t0 = time.time()
    print("Creating ephemeral Android cloud sandbox...")
    async with Sandbox.ephemeral(Image.android("14")) as sb:
        t1 = time.time()
        print(f"Sandbox ready: {sb.name} ({t1-t0:.1f}s)")

        # Screenshot
        screenshot = await sb.screenshot()
        print(f"[1/4] Screenshot: {len(screenshot)} bytes")
        assert len(screenshot) > 1000, "Screenshot too small"

        # Screen size
        w, h = await sb.screen.size()
        print(f"[2/4] Screen size: {w}x{h}")
        assert w > 0 and h > 0

        # Tap center
        cx, cy = w // 2, h // 2
        await sb.mobile.tap(cx, cy)
        print(f"[3/4] Tap ({cx}, {cy}): OK")

        # Swipe
        await sb.mobile.swipe(cx, cy + 100, cx, cy - 100, duration_ms=300)
        print("[4/4] Swipe: OK")

        print(f"\n=== All tests passed ({time.time()-t0:.1f}s total) ===")


if __name__ == "__main__":
    asyncio.run(main())
