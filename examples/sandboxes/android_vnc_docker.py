"""Android VM with live VNC viewer.

Boots Android 14 via QEMU-in-Docker, installs F-Droid, and opens VNC.

    uv run python examples/android_vnc_example.py
"""

import asyncio
import subprocess

from cua_sandbox import Image, Sandbox
from cua_sandbox.runtime import QEMURuntime

VNC_PORT = 18080
API_PORT = 18011


async def main():
    # QEMU-in-Docker runtime with VNC exposed on the host
    runtime = QEMURuntime(
        mode="docker",
        api_port=API_PORT,
        vnc_port=VNC_PORT,
        ephemeral=True,
    )

    # Android 14 + F-Droid APK (downloaded and installed via ADB post-boot)
    image = Image.android("14").apk_install("https://f-droid.org/F-Droid.apk")

    async with Sandbox.ephemeral(image, local=True, runtime=runtime, name="android-vnc-demo") as sb:
        # Open VNC viewer
        subprocess.Popen(["open", f"vnc://localhost:{VNC_PORT}"])

        # Programmatic screenshot
        screenshot = await sb.screenshot()
        with open("/tmp/android-vnc-screenshot.png", "wb") as f:
            f.write(screenshot)
        print(f"Screenshot saved ({len(screenshot)} bytes)")

        # Keep VM alive — Ctrl+C to stop
        print("Press Ctrl+C to stop.")
        try:
            while True:
                await asyncio.sleep(1)
        except KeyboardInterrupt:
            print("Shutting down...")


if __name__ == "__main__":
    asyncio.run(main())
