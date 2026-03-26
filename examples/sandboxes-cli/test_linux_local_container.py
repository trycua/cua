"""Launch a persistent local Linux container via the CLI.

    cua sb launch ubuntu:24.04 --local --json
    # parse name from JSON output, connect with SDK, run assertions
    cua sb delete <name> --local

Mirrors examples/sandboxes/test_linux_local_container.py but exercises the
CLI launch path and persistent state tracking instead of Sandbox.ephemeral().
"""

from __future__ import annotations

import asyncio
import json
import subprocess

import pytest
from cua_sandbox import Sandbox

pytestmark = pytest.mark.asyncio


def _has_docker() -> bool:
    try:
        subprocess.run(["docker", "info"], capture_output=True, check=True, timeout=10)
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


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_linux_local_container():
    result = _cua("sb", "launch", "ubuntu:24.04", "--local", "--json")
    assert result.returncode == 0, f"launch failed:\n{result.stderr}"
    name = json.loads(result.stdout)["name"]

    assert name in _ls_names(), f"'{name}' not found in `cua sb ls --all` after launch"

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("uname -s")
            assert out.success
            assert "Linux" in out.stdout

            screenshot = await sb.screenshot()
            assert screenshot[:4] == b"\x89PNG"
    finally:
        _cua("sb", "delete", name, "--local")
        assert name not in _ls_names(), f"'{name}' still in `cua sb ls --all` after delete"


async def main():
    r = _cua("sb", "launch", "ubuntu:24.04", "--local", "--json")
    print(f"launch exit={r.returncode}")
    name = json.loads(r.stdout)["name"]
    print(f"name: {name}")

    try:
        async with Sandbox.connect(name, local=True) as sb:
            out = await sb.shell.run("uname -s")
            print(f"uname: {out.stdout.strip()}")
            screenshot = await sb.screenshot()
            print(f"Screenshot: {len(screenshot)} bytes")
            with open("/tmp/cli_linux_local_container.png", "wb") as f:
                f.write(screenshot)
    finally:
        _cua("sb", "delete", name, "--local")


if __name__ == "__main__":
    asyncio.run(main())
