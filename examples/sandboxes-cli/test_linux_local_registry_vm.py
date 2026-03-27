"""Launch a persistent local Linux VM from a trycua OCI registry image via the CLI.

    cua sb launch ghcr.io/trycua/cua-xfce:latest --local --json
    # parse name from JSON output, connect with SDK, run assertions
    cua sb delete <name> --local

Mirrors examples/sandboxes/test_linux_local_registry_vm.py but exercises the
CLI launch path. Note: the SDK example uses explicit TartRuntime(); the CLI
auto-selects the runtime from the OCI manifest. On macOS with Tart installed,
pass --runtime tart to use TartRuntime instead of QEMU.
"""

from __future__ import annotations

import asyncio
import json
import platform
import subprocess

import pytest
from cua_sandbox import Sandbox

pytestmark = pytest.mark.asyncio

IS_MACOS = platform.system() == "Darwin"
IMAGE = "ghcr.io/trycua/cua-xfce:latest"


def _has_tart() -> bool:
    try:
        subprocess.run(["tart", "--version"], capture_output=True, check=True)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


def _cua(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["cua", *args], capture_output=True, text=True)


@pytest.mark.skipif(not IS_MACOS or not _has_tart(), reason="Tart only on macOS")
async def test_linux_local_registry_vm():
    result = _cua("sb", "launch", IMAGE, "--local", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("uname -s")
            assert out.success
            assert "Linux" in out.stdout

            screenshot = await sb.screenshot()
            assert screenshot[:4] == b"\x89PNG"
    finally:
        _cua("sb", "delete", name, "--local")


async def main():
    r = _cua("sb", "launch", IMAGE, "--local", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("uname -s")
            print(f"uname: {out.stdout.strip()}")
            screenshot = await sb.screenshot()
            print(f"Screenshot: {len(screenshot)} bytes")
            with open("/tmp/cli_linux_local_registry_vm.png", "wb") as f:
                f.write(screenshot)
    finally:
        _cua("sb", "delete", name, "--local")


if __name__ == "__main__":
    asyncio.run(main())
