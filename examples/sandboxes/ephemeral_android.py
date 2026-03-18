"""Ephemeral Android sandbox via the CUA platform API.

    CUA_API_KEY=sk-... uv run python examples/sandboxes/ephemeral_android.py

Available sb.mobile.* methods:

    # Touch
    sb.mobile.tap(x, y)
    sb.mobile.long_press(x, y, duration_ms=1000)
    sb.mobile.double_tap(x, y, delay=0.1)
    sb.mobile.type_text(text)

    # Gestures
    sb.mobile.swipe(x1, y1, x2, y2, duration_ms=300)
    sb.mobile.scroll_up(x, y, distance=600, duration_ms=400)
    sb.mobile.scroll_down(x, y, distance=600, duration_ms=400)
    sb.mobile.scroll_left(x, y, distance=400, duration_ms=300)
    sb.mobile.scroll_right(x, y, distance=400, duration_ms=300)
    sb.mobile.fling(x1, y1, x2, y2)
    sb.mobile.pinch_in(cx, cy, spread=300, duration_ms=400)
    sb.mobile.pinch_out(cx, cy, spread=300, duration_ms=400)

    # Hardware keys
    sb.mobile.key(keycode)
    sb.mobile.home()
    sb.mobile.back()
    sb.mobile.recents()
    sb.mobile.power()
    sb.mobile.volume_up()
    sb.mobile.volume_down()
    sb.mobile.enter()
    sb.mobile.backspace()

    # System
    sb.mobile.wake()
    sb.mobile.notifications()
    sb.mobile.close_notifications()
"""

import asyncio

from cua_sandbox import Image, sandbox


async def main():
    image = Image.android("14")

    async with sandbox(image=image) as sb:
        # Take a screenshot
        screenshot = await sb.screenshot()
        with open("/tmp/android-ephemeral-screenshot.png", "wb") as f:
            f.write(screenshot)
        print(f"Screenshot saved ({len(screenshot)} bytes)")

        # Touch interactions
        await sb.mobile.tap(540, 960)
        await sb.mobile.long_press(540, 960, duration_ms=1000)
        await sb.mobile.double_tap(540, 960)
        await sb.mobile.type_text("hello android")

        # Gestures
        await sb.mobile.swipe(540, 1500, 540, 500, duration_ms=300)
        await sb.mobile.scroll_down(540, 960)
        await sb.mobile.scroll_up(540, 960)

        # Hardware buttons
        await sb.mobile.home()
        await sb.mobile.back()
        await sb.mobile.recents()

        # Notifications
        await sb.mobile.notifications()
        await sb.mobile.close_notifications()

        # Shell still works via ADB
        result = await sb.shell.run("getprop ro.build.version.release")
        print(f"Android version: {result.stdout.strip()}")


if __name__ == "__main__":
    asyncio.run(main())
