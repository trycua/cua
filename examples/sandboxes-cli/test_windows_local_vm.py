"""Launch a persistent local Windows VM via the CLI.

    cua sb launch windows:11 --local --json
    # parse name from JSON output, connect with SDK, run assertions
    cua sb delete <name> --local

Mirrors examples/sandboxes/test_windows_local_vm.py but exercises the CLI
launch path and persistent state tracking instead of Sandbox.ephemeral().
Requires a Windows host or QEMU with a Windows image.
"""

from __future__ import annotations

import asyncio
import json
import platform
import subprocess

import pytest
from cua_sandbox import Sandbox

pytestmark = pytest.mark.asyncio

# x86_64 Windows via QEMU TCG on macOS Apple Silicon is impractically slow
# (no hardware acceleration; full install takes hours).
_IS_MACOS_ARM = platform.system() == "Darwin" and platform.machine() == "arm64"


def _has_qemu() -> bool:
    try:
        from cua_sandbox.runtime.qemu_installer import qemu_bin

        qemu_bin("x86_64")
        return True
    except Exception:
        return False


def _cua(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["cua", *args], capture_output=True, text=True)


@pytest.mark.skipif(_IS_MACOS_ARM, reason="x86_64 Windows TCG too slow on macOS Apple Silicon")
@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
async def test_windows_local_vm():
    result = _cua("sb", "launch", "windows:11", "--local", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("ver")
            assert out.success or "Windows" in out.stdout

            screenshot = await sb.screenshot()
            assert screenshot[:4] == b"\x89PNG"
    finally:
        _cua("sb", "delete", name, "--local")


async def main():
    r = _cua("sb", "launch", "windows:11", "--local", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("ver")
            print(f"ver: {out.stdout.strip()}")
            screenshot = await sb.screenshot()
            print(f"Screenshot: {len(screenshot)} bytes")
            with open("/tmp/cli_windows_local_vm.png", "wb") as f:
                f.write(screenshot)
    finally:
        _cua("sb", "delete", name, "--local")


if __name__ == "__main__":
    asyncio.run(main())
