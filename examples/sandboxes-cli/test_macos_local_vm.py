"""Launch a persistent local macOS VM via the CLI.

    cua sb launch macos --local --json
    # parse name from JSON output, connect with SDK, run assertions
    cua sb delete <name> --local

Mirrors examples/sandboxes/test_macos_local_vm.py but exercises the CLI
launch path and persistent state tracking instead of Sandbox.ephemeral().
Requires the Lume CLI on a macOS host.
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


def _has_lume() -> bool:
    try:
        subprocess.run(["lume", "--version"], capture_output=True, check=True)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


def _cua(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["cua", *args], capture_output=True, text=True)


def _ls_names() -> list[str]:
    r = _cua("sb", "ls", "--all", "--json")
    if r.returncode != 0:
        return []
    return [s["name"] for s in json.loads(r.stdout)]


@pytest.mark.skipif(not IS_MACOS or not _has_lume(), reason="Lume only on macOS")
async def test_macos_local_vm():
    result = _cua("sb", "launch", "macos", "--local", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]

    assert name in _ls_names(), f"'{name}' not found in `cua sb ls --all` after launch"

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("sw_vers")
            assert out.success
            assert "macOS" in out.stdout or "Mac" in out.stdout

            screenshot = await sb.screenshot()
            assert screenshot[:4] == b"\x89PNG"
    finally:
        _cua("sb", "delete", name, "--local")
        assert name not in _ls_names(), f"'{name}' still in `cua sb ls --all` after delete"


async def main():
    r = _cua("sb", "launch", "macos", "--local", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("sw_vers")
            print(f"sw_vers: {out.stdout.strip()}")
            screenshot = await sb.screenshot()
            print(f"Screenshot: {len(screenshot)} bytes")
            with open("/tmp/cli_macos_local_vm.png", "wb") as f:
                f.write(screenshot)
    finally:
        _cua("sb", "delete", name, "--local")


if __name__ == "__main__":
    asyncio.run(main())
