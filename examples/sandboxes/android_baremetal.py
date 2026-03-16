"""Android VM via the official Android SDK emulator (bare-metal).

Runs natively on your Mac with HVF acceleration on Apple Silicon.
Auto-installs the Android SDK, creates an AVD, and boots to homescreen.

    uv run python examples/sandboxes/android_baremetal.py
"""

import asyncio

from cua_sandbox import Image, sandbox
from cua_sandbox.runtime import AndroidEmulatorRuntime


async def main():
    # Bare-metal Android emulator — ARM64 on Apple Silicon, x86_64 on Intel
    runtime = AndroidEmulatorRuntime(memory_mb=4096, cpu_count=4, adb_port=5557, headless=False)

    # Android 14 + F-Droid installed via ADB after boot
    image = Image.android("14").apk_install("https://f-droid.org/F-Droid.apk")

    async with sandbox(
        local=True,  # Use local=False for cloud infra
        image=image,
        runtime=runtime,
        name="android-baremetal-demo",
    ) as sb:
        screenshot = await sb.screenshot()
        with open("/tmp/android-baremetal-screenshot.png", "wb") as f:
            f.write(screenshot)
        print(f"Screenshot saved ({len(screenshot)} bytes)")

        result = await sb.shell.run("getprop ro.build.version.release")
        print(f"Android version: {result.stdout.strip()}")

        print("Press Ctrl+C to stop.")
        try:
            while True:
                await asyncio.sleep(1)
        except KeyboardInterrupt:
            print("Shutting down...")


if __name__ == "__main__":
    asyncio.run(main())
