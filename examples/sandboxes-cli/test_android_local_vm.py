"""Launch a persistent local Android VM via the CLI.

    cua sb launch android:14 --local --json
    # parse name from JSON output, connect with SDK, run assertions
    cua sb delete <name> --local

Mirrors examples/sandboxes/test_android_local_vm.py but exercises the CLI
launch path and persistent state tracking instead of Sandbox.ephemeral().
Requires Android SDK (emulator + adb) to be installed.
"""

from __future__ import annotations

import asyncio
import json
import subprocess

import pytest
from cua_sandbox import Sandbox

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


@pytest.mark.skipif(not _has_android_sdk(), reason="Android SDK not available")
async def test_android_local_vm():
    result = _cua("sb", "launch", "android:14", "--local", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]

    assert name in _ls_names(), f"'{name}' not found in `cua sb ls --all` after launch"

    try:
        async with Sandbox.connect(name, local=True) as sb:
            screenshot = await sb.screenshot()
            assert screenshot[:4] == b"\x89PNG"
            assert len(screenshot) > 10000

            w, h = await sb.screen.size()
            assert w > 0 and h > 0

            await sb.mobile.home()
    finally:
        _cua("sb", "delete", name, "--local")
        assert name not in _ls_names(), f"'{name}' still in `cua sb ls --all` after delete"


async def main():
    r = _cua("sb", "launch", "android:14", "--local", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        async with Sandbox.connect(name, local=True) as sb:
            screenshot = await sb.screenshot()
            print(f"Screenshot: {len(screenshot)} bytes")
            with open("/tmp/cli_android_local_vm.png", "wb") as f:
                f.write(screenshot)
            w, h = await sb.screen.size()
            print(f"Screen size: {w}x{h}")
            await sb.mobile.tap(w // 2, h // 2)
            await sb.mobile.home()
            print("Tap + home: OK")
    finally:
        _cua("sb", "delete", name, "--local")


if __name__ == "__main__":
    asyncio.run(main())
