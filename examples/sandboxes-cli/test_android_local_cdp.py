"""Test CDP (Chrome DevTools Protocol) access via sb.tunnel on a local Android VM.

Flow:
  1. Launch android:14 via CLI
  2. Connect with SDK
  3. Open Chrome to a data: URI that seeds localStorage
  4. Forward Chrome's devtools port (9222) via sb.tunnel.forward(9222)
  5. Use CDP Runtime.evaluate to read back localStorage and assert correctness
  6. Delete sandbox

Requires:
  - Android SDK (emulator + adb)
  - Chrome / WebView debugging enabled (Android 14 emulator has it on by default)
"""

from __future__ import annotations

import asyncio
import json
import subprocess
import time

import pytest

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


def _cua(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["cua", *args], capture_output=True, text=True)


def _ls_names() -> list[str]:
    r = _cua("sb", "ls", "--all", "--json")
    if r.returncode != 0:
        return []
    return [s["name"] for s in json.loads(r.stdout)]


async def _wait_for_chrome(sb, timeout: int = 60) -> None:
    """Wait until Chrome is responding to DevTools discovery on port 9222."""
    import urllib.request

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen("http://localhost:9222/json", timeout=2) as r:
                targets = json.loads(r.read())
                if targets:
                    return
        except Exception:
            pass
        await asyncio.sleep(2)
    raise TimeoutError("Chrome DevTools not ready within timeout")


async def _cdp_evaluate(ws_url: str, expression: str) -> object:
    """Send a single CDP Runtime.evaluate and return the result value."""
    import websockets

    async with websockets.connect(ws_url) as ws:
        await ws.send(
            json.dumps(
                {
                    "id": 1,
                    "method": "Runtime.evaluate",
                    "params": {"expression": expression, "returnByValue": True},
                }
            )
        )
        raw = await asyncio.wait_for(ws.recv(), timeout=10)
        resp = json.loads(raw)
        result = resp.get("result", {}).get("result", {})
        if result.get("subtype") == "error":
            raise RuntimeError(f"CDP evaluate error: {result.get('description')}")
        return result.get("value")


@pytest.mark.skipif(not _has_android_sdk(), reason="Android SDK not available")
async def test_android_local_cdp():
    result = _cua("sb", "launch", "android:14", "--local", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]
    assert name in _ls_names()

    try:
        from cua_sandbox import Sandbox

        async with Sandbox.connect(name, local=True) as sb:
            # ── 1. Enable Chrome remote debugging ─────────────────────────────
            # Set the global WebView debug flag (works without a debug APK)
            await sb.shell.run("settings put global debug_view_attributes 1")

            # ── 2. Open Chrome to a data: page that sets localStorage ─────────
            page_html = (
                "<html><body><script>"
                "localStorage.setItem('cua_test', JSON.stringify({hello:'world',n:42}));"
                "document.title='CUA CDP Test';"
                "</script></body></html>"
            )
            import urllib.parse

            data_uri = "data:text/html," + urllib.parse.quote(page_html)
            await sb.shell.run(
                f"am start -a android.intent.action.VIEW "
                f"-n com.android.chrome/com.google.android.apps.chrome.Main "
                f"-d '{data_uri}'"
            )
            await asyncio.sleep(3)  # let Chrome load

            # ── 3. Forward devtools port via sb.tunnel ────────────────────────
            async with sb.tunnel.forward(9222) as tunnel:
                assert tunnel.port > 0
                assert tunnel.sandbox_port == 9222
                assert tunnel.url.startswith("http://localhost:")

                await _wait_for_chrome(sb, timeout=30)

                # ── 4. Discover the CDP target ────────────────────────────────
                import urllib.request

                with urllib.request.urlopen(f"http://localhost:{tunnel.port}/json", timeout=5) as r:
                    targets = json.loads(r.read())

                # Find our data: page (or any page — they all share localStorage origin)
                ws_url = targets[0]["webSocketDebuggerUrl"]
                # Replace the port in the ws URL to match our forwarded port
                ws_url = ws_url.replace(":9222/", f":{tunnel.port}/")

                # ── 5. Read localStorage via CDP ──────────────────────────────
                raw = await _cdp_evaluate(
                    ws_url,
                    "JSON.parse(localStorage.getItem('cua_test'))",
                )
                assert isinstance(raw, dict), f"Expected dict, got: {raw!r}"
                assert raw.get("hello") == "world"
                assert raw.get("n") == 42

                # ── 6. Write a new key via CDP ────────────────────────────────
                await _cdp_evaluate(
                    ws_url,
                    "localStorage.setItem('cua_written', 'via_cdp')",
                )
                written = await _cdp_evaluate(
                    ws_url,
                    "localStorage.getItem('cua_written')",
                )
                assert written == "via_cdp"

            # tunnel.forward context exited — forward rule removed
    finally:
        _cua("sb", "delete", name, "--local")
        assert name not in _ls_names()


async def main():
    """Quick manual smoke-test."""
    r = _cua("sb", "launch", "android:14", "--local", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        from cua_sandbox import Sandbox

        async with Sandbox.connect(name, local=True) as sb:
            await sb.shell.run("settings put global debug_view_attributes 1")
            await sb.shell.run(
                "am start -a android.intent.action.VIEW "
                "-n com.android.chrome/com.google.android.apps.chrome.Main "
                '-d \'data:text/html,<script>localStorage.setItem("k","v")</script>\''
            )
            await asyncio.sleep(3)

            async with sb.tunnel.forward(9222) as t:
                print(f"DevTools at {t.url}")
                await _wait_for_chrome(sb, timeout=30)
                import urllib.request

                with urllib.request.urlopen(f"{t.url}/json", timeout=5) as r:
                    targets = json.loads(r.read())
                print(f"CDP targets: {[x['url'] for x in targets]}")
                ws_url = targets[0]["webSocketDebuggerUrl"].replace(":9222/", f":{t.port}/")
                val = await _cdp_evaluate(ws_url, "localStorage.getItem('k')")
                print(f"localStorage['k'] = {val!r}")
                assert val == "v"
                print("CDP localStorage round-trip: OK")
    finally:
        _cua("sb", "delete", name, "--local")


if __name__ == "__main__":
    asyncio.run(main())
