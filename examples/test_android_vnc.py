"""Create an ephemeral Android cloud VM, take a screenshot of the VNC page, and verify."""

import asyncio
import time
from pathlib import Path

from playwright.async_api import async_playwright


async def main():
    from cua_sandbox import Image, Sandbox

    t0 = time.time()
    print("Creating ephemeral Android cloud sandbox...")

    async with Sandbox.ephemeral(Image.android("14")) as sb:
        t1 = time.time()
        name = sb.name
        print(f"Sandbox ready: {name} ({t1-t0:.1f}s)")

        vnc_url = await sb.get_display_url(share=True)
        print(f"VNC URL: {vnc_url}")

        # Take screenshot of the VNC page via Playwright
        print("Taking screenshot of VNC page...")
        async with async_playwright() as p:
            browser = await p.chromium.launch()
            page = await browser.new_page(viewport={"width": 1280, "height": 900})

            await page.goto(vnc_url, wait_until="networkidle", timeout=30000)
            await asyncio.sleep(3)  # let the stream render

            out = Path(__file__).parent / "android_vnc_screenshot.png"
            await page.screenshot(path=str(out), full_page=False)
            await browser.close()

        print(f"Screenshot saved to {out} ({out.stat().st_size} bytes)")

        # Also take a screenshot via the sandbox SDK for comparison
        sdk_screenshot = await sb.screenshot()
        sdk_out = Path(__file__).parent / "android_cloud_screenshot.png"
        sdk_out.write_bytes(sdk_screenshot)
        print(f"SDK screenshot saved to {sdk_out} ({len(sdk_screenshot)} bytes)")

        assert (
            out.stat().st_size > 10000
        ), "VNC page screenshot too small — page may not have loaded"
        assert len(sdk_screenshot) > 1000, "SDK screenshot too small"

        print(f"\n=== All checks passed ({time.time()-t0:.1f}s total) ===")
        print(f"VNC page: {vnc_url}")


if __name__ == "__main__":
    asyncio.run(main())
