"""Integration tests — runtime scenarios.

    pytest tests/test_runtime.py -v -s

Skips automatically when the required runtime isn't available.
"""

from __future__ import annotations

import platform
import subprocess

import pytest
from cua_sandbox import Image, sandbox

pytestmark = pytest.mark.asyncio

IS_WINDOWS = platform.system() == "Windows"
IS_MACOS = platform.system() == "Darwin"


# ── Helpers ──────────────────────────────────────────────────────────────────


def _has_docker() -> bool:
    try:
        subprocess.run(["docker", "info"], capture_output=True, check=True, timeout=10)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


def _has_lume() -> bool:
    try:
        subprocess.run(["lume", "--version"], capture_output=True, check=True)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


def _has_hyperv() -> bool:
    if not IS_WINDOWS:
        return False
    try:
        from cua_sandbox.runtime.hyperv import _has_hyperv as check

        return check()
    except Exception:
        return False


def _has_qemu() -> bool:
    """Check if QEMU is available (on PATH or portable)."""
    try:
        from cua_sandbox.runtime.qemu_installer import qemu_bin

        qemu_bin("x86_64")
        return True
    except (RuntimeError, Exception):
        return False


# ── 1. Linux container (Docker) ─────────────────────────────────────────────


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_linux_container():
    from cua_sandbox.runtime import DockerRuntime

    async with sandbox(
        local=True,
        image=Image.linux("ubuntu", "24.04"),
        runtime=DockerRuntime(api_port=18000, vnc_port=16901, ephemeral=True),
        name="cua-test-linux-container",
    ) as sb:
        result = await sb.shell.run("cat /etc/os-release")
        assert result.success
        assert "ubuntu" in result.stdout.lower()

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


# ── 2. Linux VM (QEMU-in-Docker) ────────────────────────────────────────────


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_linux_vm():
    from cua_sandbox.runtime import QEMURuntime

    async with sandbox(
        local=True,
        image=Image.linux("ubuntu", "24.04"),
        runtime=QEMURuntime(mode="docker", api_port=18001, vnc_port=18006, ephemeral=True),
        name="cua-test-linux-vm",
    ) as sb:
        result = await sb.shell.run("uname -a")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


# ── 3. Windows VM (bare-metal QEMU on Windows host) ─────────────────────────


@pytest.mark.skipif(not IS_WINDOWS, reason="Windows host only")
@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
async def test_windows_vm():
    from cua_sandbox.runtime import QEMURuntime

    async with sandbox(
        local=True,
        image=Image.windows("11"),
        runtime=QEMURuntime(mode="bare-metal", api_port=18002),
        name="cua-test-windows-vm",
    ) as sb:
        result = await sb.shell.run("ver")
        assert result.success or "Windows" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


# ── 3b. Windows VM (Hyper-V) ────────────────────────────────────────────────


@pytest.mark.skipif(not _has_hyperv(), reason="Hyper-V not available")
async def test_windows_hyperv():
    from cua_sandbox.runtime import HyperVRuntime

    async with sandbox(
        local=True,
        image=Image.windows("11"),
        runtime=HyperVRuntime(api_port=18003),
        name="cua-test-hyperv",
    ) as sb:
        result = await sb.shell.run("ver")
        assert result.success or "Windows" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


# ── 4. macOS VM (Lume) ──────────────────────────────────────────────────────


@pytest.mark.skipif(not IS_MACOS or not _has_lume(), reason="Lume only on macOS")
async def test_macos_vm():
    from cua_sandbox.runtime import LumeRuntime

    async with sandbox(
        local=True,
        image=Image.macos("15"),
        runtime=LumeRuntime(api_port=18005),
        name="cua-test-macos-vm",
    ) as sb:
        result = await sb.shell.run("sw_vers")
        assert result.success
        assert "macOS" in result.stdout or "Mac" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
