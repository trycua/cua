"""Test pwa_install: builds a TWA APK from a PWA manifest URL via Bubblewrap,
installs it on an Android emulator, launches it, and takes a screenshot.

Usage:
    uv run python examples/pwa_install_test.py
    uv run python examples/pwa_install_test.py --manifest https://example.com/manifest.json

Environment variables:
    CUA_ANDROID_API_LEVEL   Android API level (default: 34)
    CUA_ANDROID_AVD_NAME    AVD name          (default: cua-pwa-test)
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import urllib.request
from pathlib import Path

from cua_sandbox.image import Image
from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime
from cua_sandbox.sandbox import Sandbox

_API_LEVEL = int(os.environ.get("CUA_ANDROID_API_LEVEL", "34"))
_AVD_NAME = os.environ.get("CUA_ANDROID_AVD_NAME", "cua-pwa-test")
_SETTLE_S = 8.0
_SCREENSHOT_PATH = Path(__file__).parent / "pwa_install_screenshot.png"


def _package_id_from_manifest(manifest_url: str) -> str | None:
    """Derive the TWA package ID bubblewrap will use.

    Bubblewrap generates the package ID from the manifest host:
      https://www.starbucks.com/manifest.json  →  com.starbucks.www
    """
    from urllib.parse import urlparse

    host = urlparse(manifest_url).hostname or ""
    parts = host.split(".")
    parts.reverse()
    return ".".join(parts) if parts else None


async def run(manifest_url: str) -> None:
    print(f"\n[1/5] Fetching manifest: {manifest_url}")
    with urllib.request.urlopen(manifest_url) as resp:
        manifest = json.loads(resp.read())
    app_name = manifest.get("name") or manifest.get("short_name") or "PWA"
    print(f"      name={app_name!r}  start_url={manifest.get('start_url')!r}")

    package_id = _package_id_from_manifest(manifest_url)
    print(f"      expected package ID: {package_id}")

    print("\n[2/5] Building image with pwa_install layer …")
    image = Image.android(str(_API_LEVEL)).pwa_install(manifest_url)

    runtime = AndroidEmulatorRuntime(
        api_level=_API_LEVEL,
        memory_mb=4096,
        headless=True,
        no_boot_anim=True,
    )

    print("\n[3/5] Booting emulator and installing PWA APK (this takes a while) …")
    async with Sandbox.ephemeral(image, runtime=runtime, name=_AVD_NAME) as sb:
        # Confirm the package is installed
        result = await sb.shell.run("pm list packages -3")
        pkgs = result.stdout if hasattr(result, "stdout") else str(result)
        print(f"      Installed 3rd-party packages:\n{pkgs.strip()}")

        # Find the activity to launch
        if package_id:
            # Resolve the launcher activity from the installed package.
            resolve = await sb.shell.run(
                f"cmd package resolve-activity --brief -a android.intent.action.MAIN "
                f"-c android.intent.category.LAUNCHER {package_id}"
            )
            resolved = (resolve.stdout if hasattr(resolve, "stdout") else str(resolve)).strip()
            # Output is like "com.starbucks.www/com.starbucks.www.LauncherActivity"
            if "/" in resolved:
                component = resolved.splitlines()[-1].strip()
            else:
                # Fallback to the bubblewrap default naming convention
                component = f"{package_id}/{package_id}.LauncherActivity"
            print(f"\n[4/5] Launching {component} …")
            launch = await sb.shell.run(f"am start -n {component}")
            launched = launch.stdout if hasattr(launch, "stdout") else str(launch)
            print(f"      {launched.strip()}")
        else:
            print("\n[4/5] Could not determine package ID, skipping launch")

        await asyncio.sleep(_SETTLE_S)

        # Check foreground activity and any crash logs
        fg = await sb.shell.run("dumpsys activity activities | grep mResumedActivity")
        print(f"      Foreground: {(fg.stdout if hasattr(fg, 'stdout') else str(fg)).strip()}")
        crash = await sb.shell.run("logcat -d -s AndroidRuntime:E | grep -A5 FATAL")
        crash_out = (crash.stdout if hasattr(crash, "stdout") else str(crash)).strip()
        if crash_out:
            print(f"      CRASH LOG:\n{crash_out[:500]}")

        print(f"\n[5/5] Taking screenshot → {_SCREENSHOT_PATH}")
        screenshot = await sb.screen.screenshot()
        _SCREENSHOT_PATH.write_bytes(screenshot)
        print(f"      {len(screenshot):,} bytes saved to {_SCREENSHOT_PATH}")

    print("\nDone.")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--manifest",
        default="https://www.starbucks.com/manifest.json",
        help="URL to the PWA manifest.json",
    )
    args = parser.parse_args()
    asyncio.run(run(args.manifest))


if __name__ == "__main__":
    main()
