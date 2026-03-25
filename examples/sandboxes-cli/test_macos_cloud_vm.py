"""Launch a persistent cloud macOS VM via the CLI.

    cua sb launch macos --json
    # parse name from JSON output, connect with SDK, run assertions
    cua sb delete <name>

Mirrors examples/sandboxes/test_macos_cloud_vm.py but exercises the CLI
launch path and persistent state tracking instead of Sandbox.ephemeral().
Requires CUA_API_KEY. Works on any host OS — no Mac needed.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess

import pytest
from cua_sandbox import Sandbox

pytestmark = pytest.mark.asyncio


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


def _cua(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["cua", *args], capture_output=True, text=True)


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_macos_cloud_vm():
    result = _cua("sb", "launch", "macos", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]

    try:
        async with Sandbox.connect(name) as sb:
            out = await sb.shell.run("sw_vers")
            assert out.success
            assert "macOS" in out.stdout or "Mac" in out.stdout

            screenshot = await sb.screenshot()
            assert screenshot[:4] == b"\x89PNG"
    finally:
        _cua("sb", "delete", name)


async def main():
    r = _cua("sb", "launch", "macos", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        async with Sandbox.connect(name) as sb:
            out = await sb.shell.run("sw_vers")
            print(f"sw_vers: {out.stdout.strip()}")
            screenshot = await sb.screenshot()
            print(f"Screenshot: {len(screenshot)} bytes")
            with open("/tmp/cli_macos_cloud_vm.png", "wb") as f:
                f.write(screenshot)
    finally:
        _cua("sb", "delete", name)


if __name__ == "__main__":
    asyncio.run(main())
