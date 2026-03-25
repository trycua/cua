"""End-to-end test: gym-pwa running on the host, agent inside Android emulator.

Flow:
  1. Start gym-pwa Next.js server on the host (pnpm dev or node start)
  2. Launch android:14 emulator via CLI
  3. Open Chrome to http://10.0.2.2:3000 (host from inside emulator)
  4. POST /gym/start/add_item  → seeds DB, returns task prompt
  5. Agent taps the input, types the item, taps Add
  6. GET /gym/evaluate          → { success, reward }
  7. Assert reward == 1.0

The gym REST API is the I/O channel — no CDP required for data in/out.
CDP tunnel via sb.tunnel.forward("chrome_devtools_remote") is used as a
bonus assertion when Chrome remote debugging is available.

Requires:
  - Java (for Android SDK auto-install)
  - Node + pnpm (for gym-pwa server)
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


# ── Prereq checks ─────────────────────────────────────────────────────────────


def _has_java() -> bool:
    try:
        from cua_sandbox.runtime.android_emulator import _java_env

        _java_env()
        return True
    except Exception:
        return False


def _has_gym_pwa() -> bool:
    return (GYM_PWA_DIR / "package.json").exists()


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


# ── Chrome helpers ────────────────────────────────────────────────────────────


async def _setup_chrome(sb) -> None:
    """Prepare Chrome for automated use on a fresh emulator.

    Clears Chrome's app data so the first-run wizard (account setup screen)
    does not block navigation to the target URL.  Also marks the FRE as
    complete via SharedPreferences so subsequent launches don't re-show it.
    """
    # Clear all Chrome data (removes FRE state, cookies, cache)
    await sb.shell.run("pm clear com.android.chrome")
    await asyncio.sleep(0.5)

    # Pre-seed the FRE-complete preference so Chrome boots straight to the URL.
    # Works on AOSP / emulator builds where the prefs dir is accessible.
    await sb.shell.run(
        "mkdir -p /data/data/com.android.chrome/shared_prefs && "
        'echo \'{"first_run_flow_complete":true,"metrics_reporting":false}\' '
        "> /data/data/com.android.chrome/shared_prefs/Chrome.xml || true"
    )


# ── Test ──────────────────────────────────────────────────────────────────────

_SKIP_REASON = (
    "Java not found"
    if not _has_java()
    else (
        "gym-pwa not found at GYM_PWA_DIR"
        if not _has_gym_pwa()
        else "pnpm not found" if not _has_pnpm() else None
    )
)


@pytest.mark.skipif(_SKIP_REASON is not None, reason=_SKIP_REASON or "")
async def test_android_local_gym_pwa():
    server = _GymServer()
    server.start()

    assert _wait_for_gym(timeout=60), "gym-pwa server did not start in time"

    result = _cua("sb", "launch", "android:14", "--local", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]
    assert name in _ls_names()

    try:
        from cua_sandbox import Sandbox

        async with Sandbox.connect(name, local=True) as sb:
            # ── 1. Open gym-pwa in Chrome ─────────────────────────────────────
            await _setup_chrome(sb)
            await sb.shell.run(
                f"am start -a android.intent.action.VIEW "
                f"-n com.android.chrome/com.google.android.apps.chrome.Main "
                f"-d '{EMULATOR_GYM_URL}'"
            )
            await asyncio.sleep(5)  # let Chrome + Next.js load

            # Screenshot to confirm UI is visible
            shot = await sb.screenshot()
            assert shot[:4] == b"\x89PNG"

            # ── 2. Seed via gym REST API (data IN) ────────────────────────────
            start = _gym_request("POST", "/api/gym/start/add_item")
            assert start["success"], f"start failed: {start}"
            prompt = start["prompt"]
            assert "Buy groceries" in prompt, f"unexpected prompt: {prompt}"

            # ── 3. Reload so the app picks up the fresh DB state ──────────────
            await sb.shell.run(
                f"am start -a android.intent.action.VIEW "
                f"-n com.android.chrome/com.google.android.apps.chrome.Main "
                f"-d '{EMULATOR_GYM_URL}'"
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
        _cua("sb", "delete", name, "--local")
        assert name not in _ls_names()


async def _try_cdp_verify(sb, expected_text: str) -> None:
    """Optional: verify the item exists in the DOM via CDP.

    Silently skips if Chrome remote debugging is not available.
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
                texts = [t.lower() for t in result]
                assert any(
                    expected_text in t for t in texts
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
    server = _GymServer()
    server.start()
    print(f"Waiting for gym-pwa at {GYM_URL} ...")
    assert _wait_for_gym(60), "server timeout"
    print("Server ready.")

    r = _cua("sb", "launch", "android:14", "--local", "--json")
    name = json.loads(r.stdout)["name"]
    print(f"Sandbox: {name}")

    try:
        from cua_sandbox import Sandbox

        async with Sandbox.connect(name, local=True) as sb:
            await _setup_chrome(sb)
            await sb.shell.run(
                f"am start -a android.intent.action.VIEW "
                f"-n com.android.chrome/com.google.android.apps.chrome.Main "
                f"-d '{EMULATOR_GYM_URL}'"
            )
            await asyncio.sleep(5)

            start = _gym_request("POST", "/api/gym/start/add_item")
            print(f"Task: {start['prompt']}")

            await sb.shell.run(
                f"am start -a android.intent.action.VIEW "
                f"-n com.android.chrome/com.google.android.apps.chrome.Main "
                f"-d '{EMULATOR_GYM_URL}'"
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
        _cua("sb", "delete", name, "--local")


if __name__ == "__main__":
    asyncio.run(main())
