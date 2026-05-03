"""
Demo 2: Ephemeral Android Sandbox — F-Droid APK Install
Spins up a local Android VM, installs F-Droid, takes a screenshot.
Sandbox is automatically destroyed on exit.

Usage:
    python demo/2_ephemeral_fdroid.py
"""

import asyncio
from pathlib import Path

from cua_sandbox import Sandbox
from cua_sandbox.image import Image

FDROID_APK = "https://f-droid.org/F-Droid.apk"

OUT_DIR = Path(__file__).parent / "out"


async def main():
    OUT_DIR.mkdir(exist_ok=True)

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
        out = OUT_DIR / "fdroid_home.png"
        out.write_bytes(screenshot)
        print(f"  Screenshot  : {out}  ({len(screenshot):,} bytes)")

        print("\nLaunching F-Droid...")
        await sb.shell.run("am start -n org.fdroid.fdroid/.views.main.MainActivity")
        await asyncio.sleep(3)

        screenshot2 = await sb.screenshot(format="png")
        out2 = OUT_DIR / "fdroid_launched.png"
        out2.write_bytes(screenshot2)
        print(f"  Screenshot  : {out2}  ({len(screenshot2):,} bytes)")

    print("\n✓ Sandbox destroyed\n")
    print("=" * 50 + "\n")


if __name__ == "__main__":
    asyncio.run(main())
