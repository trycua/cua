"""E2E test: Windows display URL via shared RDP gateway.

Tests the display URL flow for Windows VMs:
- get_display_url(share=False) returns the dashboard URL
- get_display_url(share=True) returns a gateway URL with encrypted token
- Playwright loads the gateway URL and verifies the desktop renders

Run against local dev (creates a new VM):
    CUA_API_KEY=sk-dev-test-key-local-12345 CUA_BASE_URL=http://localhost:8082 \
    uv run pytest tests/test_windows_display_url.py -v -s

Run against an existing VM (skip creation):
    CUA_API_KEY=sk-... CUA_TEST_WINDOWS_VM=fuzzy-penguin \
    uv run pytest tests/test_windows_display_url.py -v -s

Requires: playwright (`pip install playwright && playwright install chromium`)
"""

from __future__ import annotations

import os
import time
from pathlib import Path

import pytest

from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio
P = lambda *a, **kw: print(*a, **kw, flush=True)

SCREENSHOT_DIR = Path(__file__).parent / "screenshots"
SCREENSHOT_DIR.mkdir(exist_ok=True)

skip_no_key = pytest.mark.skipif(
    not os.environ.get("CUA_API_KEY"), reason="CUA_API_KEY not set"
)

# If set, connect to an existing Windows VM instead of creating one
EXISTING_VM = os.environ.get("CUA_TEST_WINDOWS_VM")


async def _get_windows_sandbox():
    """Get a Windows sandbox — either connect to existing or create ephemeral."""
    if EXISTING_VM:
        P(f"\n  Connecting to existing VM: {EXISTING_VM}")
        sb = await Sandbox.connect(EXISTING_VM)
        return sb, False  # not ephemeral
    P("\n  Creating ephemeral Windows VM...")
    sb = await Sandbox._create(image=Image.windows("server-2025"))
    return sb, True


@skip_no_key
async def test_windows_display_url_share_false():
    """get_display_url(share=False) returns the dashboard connect URL."""
    sb, ephemeral = await _get_windows_sandbox()
    try:
        P(f"  VM: {sb.name}")
        url = await sb.get_display_url(share=False)
        P(f"  display_url(share=False): {url}")
        assert sb.name in url
        assert "connect/" in url
    finally:
        if ephemeral:
            await sb.destroy()
        else:
            await sb.disconnect()


@skip_no_key
async def test_windows_display_url_share_true():
    """get_display_url(share=True) returns a gateway URL with encrypted token."""
    sb, ephemeral = await _get_windows_sandbox()
    try:
        P(f"  VM: {sb.name}")
        url = await sb.get_display_url(share=True)
        P(f"  display_url(share=True): {url}")
        assert "token=" in url, f"Shareable URL must contain encrypted token, got: {url}"
    finally:
        if ephemeral:
            await sb.destroy()
        else:
            await sb.disconnect()


@skip_no_key
async def test_windows_gateway_shows_desktop():
    """Navigate to the shareable gateway URL and verify the desktop renders.

    Uses Playwright headless Chromium to load the gateway page, waits for the
    Guacamole canvas to appear, then takes a screenshot to confirm the Windows
    desktop is visible.
    """
    try:
        from playwright.async_api import async_playwright
    except ImportError:
        pytest.skip("playwright not installed")

    sb, ephemeral = await _get_windows_sandbox()
    try:
        P(f"  VM: {sb.name}")

        # Verify CUA server is responsive via SDK
        t0 = time.monotonic()
        screen = await sb.screenshot()
        P(f"  SDK screenshot: {len(screen)} bytes ({time.monotonic() - t0:.1f}s)")
        sdk_path = SCREENSHOT_DIR / "windows_sdk_screenshot.png"
        sdk_path.write_bytes(screen)
        P(f"  Saved: {sdk_path}")
        assert len(screen) > 1000

        # Get shareable display URL
        display_url = await sb.get_display_url(share=True)
        P(f"  display_url: {display_url}")
        assert "token=" in display_url

        # Load in Playwright
        async with async_playwright() as pw:
            browser = await pw.chromium.launch(headless=True)
            page = await browser.new_page(viewport={"width": 1920, "height": 1080})

            P("  Loading gateway page...")
            await page.goto(display_url, wait_until="networkidle", timeout=30000)

            initial_path = SCREENSHOT_DIR / "windows_gateway_initial.png"
            await page.screenshot(path=str(initial_path))
            P(f"  Saved: {initial_path}")

            # Wait for Guacamole canvas
            P("  Waiting for canvas...")
            try:
                await page.wait_for_selector("canvas", timeout=30000)
            except Exception as e:
                err_path = SCREENSHOT_DIR / "windows_gateway_error.png"
                await page.screenshot(path=str(err_path))
                content = await page.content()
                P(f"  Page HTML (500 chars): {content[:500]}")
                pytest.fail(f"Guacamole canvas not found: {e}")

            # Wait for RDP desktop to render
            P("  Waiting 15s for RDP desktop...")
            await page.wait_for_timeout(15000)

            desktop_path = SCREENSHOT_DIR / "windows_gateway_desktop.png"
            await page.screenshot(path=str(desktop_path))
            P(f"  Saved: {desktop_path}")

            canvas_box = await page.locator("canvas").first.bounding_box()
            assert canvas_box is not None, "Canvas should be visible"
            assert canvas_box["width"] > 100
            assert canvas_box["height"] > 100
            P(f"  Canvas: {canvas_box['width']}x{canvas_box['height']}")

            # Check canvas has non-trivial content (not all black)
            has_content = await page.evaluate("""() => {
                const c = document.querySelector('canvas');
                if (!c) return false;
                const ctx = c.getContext('2d');
                if (!ctx) return false;
                const w = Math.min(c.width, 200), h = Math.min(c.height, 200);
                const d = ctx.getImageData(0, 0, w, h).data;
                let n = 0;
                for (let i = 0; i < d.length; i += 4)
                    if (d[i] + d[i+1] + d[i+2] > 30) n++;
                return n > w * h * 0.05;
            }""")

            if not has_content:
                P("  Canvas blank, waiting 15s more...")
                await page.wait_for_timeout(15000)
                retry_path = SCREENSHOT_DIR / "windows_gateway_retry.png"
                await page.screenshot(path=str(retry_path))
                P(f"  Saved: {retry_path}")

                has_content = await page.evaluate("""() => {
                    const c = document.querySelector('canvas');
                    if (!c) return false;
                    const ctx = c.getContext('2d');
                    if (!ctx) return false;
                    const w = Math.min(c.width, 200), h = Math.min(c.height, 200);
                    const d = ctx.getImageData(0, 0, w, h).data;
                    let n = 0;
                    for (let i = 0; i < d.length; i += 4)
                        if (d[i] + d[i+1] + d[i+2] > 30) n++;
                    return n > w * h * 0.05;
                }""")

            assert has_content, (
                f"Gateway canvas is blank — desktop not rendering. "
                f"Screenshots: {SCREENSHOT_DIR}"
            )
            P("  Desktop is rendering!")
            await browser.close()
    finally:
        if ephemeral:
            await sb.destroy()
        else:
            await sb.disconnect()

    P(f"\n  All screenshots in {SCREENSHOT_DIR}/")
