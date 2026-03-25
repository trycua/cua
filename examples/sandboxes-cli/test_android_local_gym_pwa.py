"""End-to-end test: gym-pwa on Modal, agent inside Android emulator as TWA.

Flow:
  1. Build + cache TWA APK from the gym-pwa keystore (bundled in repo)
  2. Launch android:14 emulator, install the signed TWA APK via pwa_install
  3. Launch TWA app — no Chrome FRE, no browser UI
  4. POST /gym/start/add_item  → seeds DB, returns task prompt
  5. Agent taps the input, types the item, taps Add
  6. GET /gym/evaluate          → { success, reward }
  7. Assert reward == 1.0

The gym REST API (hosted on Modal) is the I/O channel.
The TWA is signed with android.keystore from the gym-pwa repo; the Modal
deployment serves the matching fingerprint from /.well-known/assetlinks.json.

Requires:
  - Java (for Android SDK auto-install + keytool)
  - Node + npm (for bubblewrap)
  - gym-pwa repo checked out at GYM_PWA_DIR (defaults to ~/gym-pwa)
    — only needed for the android.keystore file
"""

from __future__ import annotations

import asyncio
import json
import os
import urllib.request
from pathlib import Path

import pytest

pytestmark = pytest.mark.asyncio

# ── Config ────────────────────────────────────────────────────────────────────

GYM_URL = os.environ.get("GYM_URL", "https://cuaai--todo-gym-web.modal.run")
GYM_PWA_DIR = Path(os.environ.get("GYM_PWA_DIR", Path.home() / "gym-pwa"))
TWA_PACKAGE = "com.cuaai.gymtodo"
TWA_MANIFEST_URL = f"{GYM_URL}/manifest.json"
GYM_KEYSTORE = GYM_PWA_DIR / "android.keystore"


# ── Prereq checks ─────────────────────────────────────────────────────────────


def _has_java() -> bool:
    try:
        from cua_sandbox.runtime.android_emulator import _java_env

        _java_env()
        return True
    except Exception:
        return False


def _has_keystore() -> bool:
    return GYM_KEYSTORE.exists()


# ── Gym API helpers ───────────────────────────────────────────────────────────


def _gym_request(method: str, path: str, body: dict | None = None) -> dict:
    url = f"{GYM_URL}{path}"
    data = json.dumps(body).encode() if body else None
    req = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={"Content-Type": "application/json"} if data else {},
    )
    with urllib.request.urlopen(req, timeout=15) as r:
        return json.loads(r.read())


# ── Test ──────────────────────────────────────────────────────────────────────

_SKIP_REASON = (
    "Java not found"
    if not _has_java()
    else (
        f"android.keystore not found at {GYM_KEYSTORE} (clone trycua/android-example-gym-pwa-app)"
        if not _has_keystore()
        else None
    )
)


@pytest.mark.skipif(_SKIP_REASON is not None, reason=_SKIP_REASON or "")
async def test_android_local_gym_pwa():
    from cua_sandbox import Sandbox
    from cua_sandbox.image import Image

    async with Sandbox.ephemeral(
        Image.android().pwa_install(
            TWA_MANIFEST_URL,
            package_name=TWA_PACKAGE,
            keystore=str(GYM_KEYSTORE),
        ),
        local=True,
    ) as sb:
        # ── 1. Launch TWA (no Chrome FRE / browser UI) ────────────────────────
        await sb.shell.run(
            f"am start -n {TWA_PACKAGE}/.LauncherActivity "
            f"-a android.intent.action.MAIN "
            f"-c android.intent.category.LAUNCHER"
        )
        await asyncio.sleep(5)

        shot = await sb.screenshot()
        assert shot[:4] == b"\x89PNG"

        # ── 2. Seed via gym REST API ──────────────────────────────────────────
        start = _gym_request("POST", "/api/gym/start/add_item")
        assert start["success"], f"start failed: {start}"
        assert "Buy groceries" in start["prompt"], f"unexpected prompt: {start['prompt']}"

        # ── 3. Reload TWA so the app picks up fresh DB state ─────────────────
        await sb.shell.run(
            f"am start -n {TWA_PACKAGE}/.LauncherActivity "
            f"-a android.intent.action.MAIN "
            f"-c android.intent.category.LAUNCHER"
        )
        await asyncio.sleep(3)

        # ── 4. Agent: tap input, type, tap Add ────────────────────────────────
        w, h = await sb.screen.size()
        await sb.mobile.tap(w // 2, int(h * 0.18))
        await asyncio.sleep(0.5)
        await sb.mobile.type_text("Buy groceries")
        await asyncio.sleep(0.3)
        await sb.mobile.tap(int(w * 0.85), int(h * 0.18))
        await asyncio.sleep(1)

        # ── 5. Evaluate ───────────────────────────────────────────────────────
        result = _gym_request("GET", "/api/gym/evaluate")
        assert result["success"], f"eval failed: reward={result['reward']}, msg={result['message']}"
        assert result["reward"] == 1.0, f"expected reward 1.0, got {result['reward']}"

        # ── 6. Bonus: CDP via tunnel if available ─────────────────────────────
        await _try_cdp_verify(sb, "buy groceries")


async def _try_cdp_verify(sb, expected_text: str) -> None:
    """Optional DOM check via CDP; silently skips if unavailable."""
    try:
        async with sb.tunnel.forward("chrome_devtools_remote") as t:
            with urllib.request.urlopen(f"{t.url}/json", timeout=3) as r:
                targets = json.loads(r.read())
            if not targets:
                return
            ws_url = targets[0]["webSocketDebuggerUrl"].replace(
                "localhost/", f"localhost:{t.port}/"
            )
            result = await _cdp_evaluate(
                ws_url,
                "Array.from(document.querySelectorAll('li span')).map(e=>e.textContent)",
            )
            if isinstance(result, list):
                texts = [item.lower() for item in result]
                assert any(
                    expected_text in item for item in texts
                ), f"CDP: '{expected_text}' not found in {result}"
    except Exception:
        pass


async def _cdp_evaluate(ws_url: str, expression: str) -> object:
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
        return resp.get("result", {}).get("result", {}).get("value")


# ── Manual runner ─────────────────────────────────────────────────────────────


async def main():
    from cua_sandbox import Sandbox
    from cua_sandbox.image import Image

    print(f"Using gym at {GYM_URL}")
    print(f"Building TWA APK from {TWA_MANIFEST_URL} ...")

    async with Sandbox.ephemeral(
        Image.android().pwa_install(
            TWA_MANIFEST_URL,
            package_name=TWA_PACKAGE,
            keystore=str(GYM_KEYSTORE),
        ),
        local=True,
    ) as sb:
        await sb.shell.run(
            f"am start -n {TWA_PACKAGE}/.LauncherActivity "
            f"-a android.intent.action.MAIN "
            f"-c android.intent.category.LAUNCHER"
        )
        await asyncio.sleep(5)

        start = _gym_request("POST", "/api/gym/start/add_item")
        print(f"Task: {start['prompt']}")

        await sb.shell.run(
            f"am start -n {TWA_PACKAGE}/.LauncherActivity "
            f"-a android.intent.action.MAIN "
            f"-c android.intent.category.LAUNCHER"
        )
        await asyncio.sleep(3)

        w, h = await sb.screen.size()
        await sb.mobile.tap(w // 2, int(h * 0.18))
        await asyncio.sleep(0.5)
        await sb.mobile.type_text("Buy groceries")
        await asyncio.sleep(0.3)
        await sb.mobile.tap(int(w * 0.85), int(h * 0.18))
        await asyncio.sleep(1)

        shot = await sb.screenshot()
        with open("/tmp/gym_pwa_after_add.png", "wb") as f:
            f.write(shot)
        print("Screenshot saved to /tmp/gym_pwa_after_add.png")

        result = _gym_request("GET", "/api/gym/evaluate")
        print(
            f"Eval: success={result['success']} reward={result['reward']} msg={result['message']}"
        )


if __name__ == "__main__":
    asyncio.run(main())
