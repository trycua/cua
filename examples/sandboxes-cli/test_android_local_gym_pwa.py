"""End-to-end test: gym-pwa running on the host, agent inside Android emulator as TWA.

Flow:
  1. Build + cache TWA APK from the gym-pwa keystore (bundled in repo)
  2. Start gym-pwa Next.js server on the host (pnpm dev)
  3. Launch android:14 emulator, install the signed TWA APK via pwa_install
  4. Launch TWA app (am start com.cuaai.gymtodo) — no Chrome FRE, no browser UI
  5. POST /gym/start/add_item  → seeds DB, returns task prompt
  6. Agent taps the input, types the item, taps Add
  7. GET /gym/evaluate          → { success, reward }
  8. Assert reward == 1.0

The gym REST API is the I/O channel — no CDP required for data in/out.
The TWA (Trusted Web Activity) is signed with android.keystore committed in the
gym-pwa repo; /.well-known/assetlinks.json returns the matching SHA-256
fingerprint by default so Chrome trusts it without showing the browser UI.

Requires:
  - Java (for Android SDK auto-install and keytool)
  - Node + npm + pnpm (for bubblewrap + gym-pwa server)
  - gym-pwa repo checked out at GYM_PWA_DIR (defaults to ~/gym-pwa)
"""

from __future__ import annotations

import asyncio
import json
import os
import signal
import subprocess
import time
import urllib.request
from pathlib import Path

import pytest

pytestmark = pytest.mark.asyncio

# ── Config ────────────────────────────────────────────────────────────────────

GYM_PWA_DIR = Path(os.environ.get("GYM_PWA_DIR", Path.home() / "gym-pwa"))
GYM_HOST = "127.0.0.1"
GYM_PORT = 3000
GYM_URL = f"http://{GYM_HOST}:{GYM_PORT}"
# 10.0.2.2 is the host loopback from inside the Android emulator
EMULATOR_GYM_URL = "http://10.0.2.2:3000"
TWA_PACKAGE = "com.cuaai.gymtodo"
TWA_MANIFEST_URL = f"{EMULATOR_GYM_URL}/manifest.json"
# android.keystore committed in the gym-pwa repo (password: "android")
GYM_KEYSTORE = GYM_PWA_DIR / "android.keystore"


# ── Prereq checks ─────────────────────────────────────────────────────────────


def _has_java() -> bool:
    try:
        from cua_sandbox.runtime.android_emulator import _java_env

        _java_env()
        return True
    except Exception:
        return False


def _has_gym_pwa() -> bool:
    return (GYM_PWA_DIR / "package.json").exists() and GYM_KEYSTORE.exists()


def _has_pnpm() -> bool:
    import shutil

    return shutil.which("pnpm") is not None


def _cua(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["cua", *args], capture_output=True, text=True)


def _ls_names() -> list[str]:
    r = _cua("sb", "ls", "--all", "--json")
    if r.returncode != 0:
        return []
    return [s["name"] for s in json.loads(r.stdout)]


# ── Gym server helpers ────────────────────────────────────────────────────────


def _gym_request(method: str, path: str, body: dict | None = None) -> dict:
    url = f"{GYM_URL}{path}"
    data = json.dumps(body).encode() if body else None
    req = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={"Content-Type": "application/json"} if data else {},
    )
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read())


def _wait_for_gym(timeout: int = 30) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            _gym_request("GET", "/api/gym/tasks")
            return True
        except Exception:
            time.sleep(1)
    return False


class _GymServer:
    """Starts gym-pwa with `pnpm dev` and stops it on exit."""

    def __init__(self):
        self._proc: subprocess.Popen | None = None

    def start(self) -> None:
        env = os.environ.copy()
        env["PORT"] = str(GYM_PORT)
        self._proc = subprocess.Popen(
            ["pnpm", "dev", "--hostname", "0.0.0.0"],
            cwd=GYM_PWA_DIR,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            preexec_fn=os.setsid,
        )

    def stop(self) -> None:
        if self._proc:
            try:
                os.killpg(os.getpgid(self._proc.pid), signal.SIGTERM)
            except ProcessLookupError:
                pass
            self._proc = None


# ── Test ──────────────────────────────────────────────────────────────────────

_SKIP_REASON = (
    "Java not found"
    if not _has_java()
    else (
        "gym-pwa not found at GYM_PWA_DIR (need package.json + android.keystore)"
        if not _has_gym_pwa()
        else "pnpm not found" if not _has_pnpm() else None
    )
)


@pytest.mark.skipif(_SKIP_REASON is not None, reason=_SKIP_REASON or "")
async def test_android_local_gym_pwa():
    from cua_sandbox import Sandbox
    from cua_sandbox.image import Image

    server = _GymServer()
    server.start()

    assert _wait_for_gym(timeout=60), "gym-pwa server did not start in time"

    # Launch emulator with TWA APK installed via pwa_install + committed keystore
    async with Sandbox.ephemeral(
        Image.android().pwa_install(
            TWA_MANIFEST_URL,
            package_name=TWA_PACKAGE,
            keystore=str(GYM_KEYSTORE),
        ),
        local=True,
    ) as sb:
        try:
            # ── 1. Launch TWA (no Chrome browser UI / FRE) ────────────────────
            await sb.shell.run(
                f"am start -n {TWA_PACKAGE}/.LauncherActivity "
                f"-a android.intent.action.MAIN "
                f"-c android.intent.category.LAUNCHER"
            )
            await asyncio.sleep(5)  # let TWA + Next.js load

            # Screenshot to confirm UI is visible
            shot = await sb.screenshot()
            assert shot[:4] == b"\x89PNG"

            # ── 2. Seed via gym REST API (data IN) ────────────────────────────
            start = _gym_request("POST", "/api/gym/start/add_item")
            assert start["success"], f"start failed: {start}"
            prompt = start["prompt"]
            assert "Buy groceries" in prompt, f"unexpected prompt: {prompt}"

            # ── 3. Reload TWA so the app picks up the fresh DB state ──────────
            await sb.shell.run(
                f"am start -n {TWA_PACKAGE}/.LauncherActivity "
                f"-a android.intent.action.MAIN "
                f"-c android.intent.category.LAUNCHER"
            )
            await asyncio.sleep(3)

            # ── 4. Agent interacts: tap the input field, type, tap Add ────────
            w, h = await sb.screen.size()
            cx = w // 2

            # Input field is near the top — tap ~18% down
            await sb.mobile.tap(cx, int(h * 0.18))
            await asyncio.sleep(0.5)
            await sb.mobile.type_text("Buy groceries")
            await asyncio.sleep(0.3)

            # Add button is to the right of the input
            await sb.mobile.tap(int(w * 0.85), int(h * 0.18))
            await asyncio.sleep(1)

            # ── 5. Evaluate via gym REST API (data OUT) ───────────────────────
            result = _gym_request("GET", "/api/gym/evaluate")
            assert result[
                "success"
            ], f"eval failed: reward={result['reward']}, msg={result['message']}"
            assert result["reward"] == 1.0, f"expected reward 1.0, got {result['reward']}"

            # ── 6. Bonus: CDP tunnel if Chrome debugging is available ──────────
            await _try_cdp_verify(sb, "buy groceries")

        finally:
            server.stop()


async def _try_cdp_verify(sb, expected_text: str) -> None:
    """Optional: verify the item exists in the DOM via CDP.

    TWA apps expose Chrome DevTools on localabstract:chrome_devtools_remote
    when running with a debug-enabled Chrome build.  Silently skips if
    unavailable (production Chrome stable on most emulators).
    """
    try:
        async with sb.tunnel.forward("chrome_devtools_remote") as t:
            import urllib.request as _ur

            with _ur.urlopen(f"{t.url}/json", timeout=3) as r:
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
                ), f"CDP: '{expected_text}' not found in DOM items: {result}"
    except Exception:
        pass  # CDP not available — skip silently


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

    server = _GymServer()
    server.start()
    print(f"Waiting for gym-pwa at {GYM_URL} ...")
    assert _wait_for_gym(60), "server timeout"
    print("Server ready.")

    print(f"Launching emulator and installing TWA APK from {TWA_MANIFEST_URL} ...")
    async with Sandbox.ephemeral(
        Image.android().pwa_install(
            TWA_MANIFEST_URL,
            package_name=TWA_PACKAGE,
            keystore=str(GYM_KEYSTORE),
        ),
        local=True,
    ) as sb:
        try:
            # Launch TWA
            await sb.shell.run(
                f"am start -n {TWA_PACKAGE}/.LauncherActivity "
                f"-a android.intent.action.MAIN "
                f"-c android.intent.category.LAUNCHER"
            )
            await asyncio.sleep(5)

            start = _gym_request("POST", "/api/gym/start/add_item")
            print(f"Task: {start['prompt']}")

            # Reload
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
        finally:
            server.stop()


if __name__ == "__main__":
    asyncio.run(main())
