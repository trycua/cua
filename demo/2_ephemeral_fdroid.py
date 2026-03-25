"""
Demo 2: Ephemeral Android Sandbox — F-Droid APK Install
Spins up a local Android VM, installs F-Droid, takes a screenshot.
Sandbox is automatically destroyed on exit.

Usage:
    python demo/2_ephemeral_fdroid.py
"""

import asyncio

from cua_sandbox import Sandbox
from cua_sandbox.image import Image

FDROID_APK = "https://f-droid.org/F-Droid.apk"


async def main():
    print("\n" + "=" * 50)
    print("  Ephemeral Android Sandbox — F-Droid")
    print("=" * 50 + "\n")

    image = Image.android().apk_install(FDROID_APK)

    print("Starting local Android VM with F-Droid pre-installed...")
    print("(VM is ephemeral — destroyed automatically on exit)\n")

    async with Sandbox.ephemeral(image, local=True) as sb:
        print("✓ Sandbox ready\n")

        w, h = await sb.screen.size()
        print(f"  Screen size : {w}x{h}")

        screenshot = await sb.screenshot(format="png")
        out = "/tmp/fdroid_demo.png"
        with open(out, "wb") as f:
            f.write(screenshot)
        print(f"  Screenshot  : saved to {out}  ({len(screenshot):,} bytes)")

        print("\nLaunching F-Droid...")
        await sb.shell.run("am start -n org.fdroid.fdroid/.views.main.MainActivity")
        await asyncio.sleep(3)

        screenshot2 = await sb.screenshot(format="png")
        out2 = "/tmp/fdroid_demo_launched.png"
        with open(out2, "wb") as f:
            f.write(screenshot2)
        print(f"  Screenshot  : saved to {out2}  ({len(screenshot2):,} bytes)")

    print("\n✓ Sandbox destroyed\n")
    print("=" * 50 + "\n")


if __name__ == "__main__":
    asyncio.run(main())
