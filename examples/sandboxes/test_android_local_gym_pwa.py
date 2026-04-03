"""End-to-end test: gym-pwa on Modal, agent inside Android emulator as WebView app.

Flow:
  1. Build + cache WebView APK from the PWA manifest via pwa2apk (no Chrome dependency)
  2. Launch android:14 emulator, install the signed APK via pwa_install(builder="pwa2apk")
  3. POST /gym/start/add_item  → seeds DB, returns session_id + task prompt
  4. Launch app with session URL via deep-link intent — no Chrome banner, no FRE
  5. Agent taps the input, types the item, taps Add
  6. GET /gym/evaluate (x-session-id header) → { success, reward }
  7. Assert reward == 1.0

The gym REST API (hosted on Modal) is the I/O channel.
pwa2apk generates a WebView APK — no assetlinks.json required.

Requires:
  - Java (for Android SDK auto-install + keytool)
  - gym-pwa repo checked out at GYM_PWA_DIR (defaults to ~/gym-pwa)
    — only needed for the android.keystore file
"""

from __future__ import annotations

import asyncio
import json
import os
import urllib.parse
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
BG_COLOR = "#dbe4ff"  # light indigo tint — visually distinct without obscuring UI


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


def _gym_request(
    method: str, path: str, body: dict | None = None, session_id: str | None = None
) -> dict:
    url = f"{GYM_URL}{path}"
    data = json.dumps(body).encode() if body else None
    hdrs: dict[str, str] = {}
    if data:
        hdrs["Content-Type"] = "application/json"
    if session_id:
        hdrs["x-session-id"] = session_id
    req = urllib.request.Request(url, data=data, method=method, headers=hdrs)
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
            builder="pwa2apk",  # WebView APK — no Chrome, no banner
        ),
        local=True,
    ) as sb:
        # ── 1. Seed via gym REST API (creates a fresh session + bg color) ─────
        start = _gym_request("POST", "/api/gym/start/add_item", body={"bgColor": BG_COLOR})
        assert start["success"], f"start failed: {start}"
        assert "Buy groceries" in start["prompt"], f"unexpected prompt: {start['prompt']}"
        session_id = start["sessionId"]

        session_url = f"{GYM_URL}/?session={session_id}&bg={urllib.parse.quote(BG_COLOR, safe='')}"

        # ── 2. Launch app directly to the session URL via VIEW intent ─────────
        # pwa2apk WebView apps handle http/https VIEW intents natively.
        await sb.shell.run(
            f"am start -a android.intent.action.VIEW "
            f"-d '{session_url}' "
            f"-n {TWA_PACKAGE}/.MainActivity"
        )

        # Wait for WebView to load the React app
        await asyncio.sleep(8)

        shot = await sb.screenshot()
        assert shot[:4] == b"\x89PNG"
        with open("/tmp/gym_pwa_pre_agent.png", "wb") as f:
            f.write(shot)

        # Verify no Chrome disclosure banner in the UI
        await sb.shell.run("uiautomator dump /sdcard/ui.xml")
        dump = await sb.shell.run("cat /sdcard/ui.xml")
        assert "Running in Chrome" not in dump.stdout, "Should not show Chrome banner"

        # ── 3. Agent: tap input, type item, tap Add ───────────────────────────
        w, h = await sb.screen.size()
        await sb.mobile.tap(w // 2, int(h * 0.18))
        await asyncio.sleep(0.5)
        await sb.mobile.type_text("Buy groceries")
        await asyncio.sleep(0.3)
        await sb.mobile.tap(int(w * 0.85), int(h * 0.18))
        await asyncio.sleep(1)

        # ── 4. Evaluate via REST API ──────────────────────────────────────────
        result = _gym_request("GET", "/api/gym/evaluate", session_id=session_id)
        assert result["success"], f"eval failed: reward={result['reward']}, msg={result['message']}"
        assert result["reward"] == 1.0, f"expected reward 1.0, got {result['reward']}"


# ── Manual runner ─────────────────────────────────────────────────────────────


async def main():
    from cua_sandbox import Sandbox
    from cua_sandbox.image import Image

    print(f"Using gym at {GYM_URL}")
    print(f"Building WebView APK from {TWA_MANIFEST_URL} via pwa2apk ...")

    async with Sandbox.ephemeral(
        Image.android().pwa_install(
            TWA_MANIFEST_URL,
            package_name=TWA_PACKAGE,
            keystore=str(GYM_KEYSTORE),
            builder="pwa2apk",
        ),
        local=True,
    ) as sb:
        start = _gym_request("POST", "/api/gym/start/add_item", body={"bgColor": BG_COLOR})
        session_id = start["sessionId"]
        session_url = f"{GYM_URL}/?session={session_id}&bg={urllib.parse.quote(BG_COLOR, safe='')}"
        print(f"Task: {start['prompt']}  session: {session_id}  bg: {BG_COLOR}")

        await sb.shell.run(
            f"am start -a android.intent.action.VIEW "
            f"-d '{session_url}' "
            f"-n {TWA_PACKAGE}/.MainActivity"
        )
        await asyncio.sleep(8)

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

        result = _gym_request("GET", "/api/gym/evaluate", session_id=session_id)
        print(
            f"Eval: success={result['success']} reward={result['reward']} msg={result['message']}"
        )


if __name__ == "__main__":
    asyncio.run(main())
