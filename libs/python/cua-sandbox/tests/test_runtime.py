"""Integration tests — runtime scenarios.

    pytest tests/test_runtime.py -v -s

Skips automatically when the required runtime isn't available.
"""

from __future__ import annotations

import os
import platform
import subprocess
from pathlib import Path

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

IS_WINDOWS = platform.system() == "Windows"
IS_MACOS = platform.system() == "Darwin"


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


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


def _has_android_image() -> bool:
    """Check if an Android-x86 disk image is available for bare-metal QEMU."""
    from pathlib import Path

    storage = Path.home() / ".cua" / "cua-sandbox" / "qemu-storage" / "android-x86.qcow2"
    return storage.exists()


# ── 1. Linux container (Docker) ─────────────────────────────────────────────


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_linux_container():
    from cua_sandbox.runtime import DockerRuntime

    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        local=True,
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

    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        local=True,
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

    async with Sandbox.ephemeral(
        Image.windows("11"),
        local=True,
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

    async with Sandbox.ephemeral(
        Image.windows("11"),
        local=True,
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

    async with Sandbox.ephemeral(
        Image.macos("26"),
        local=True,
        runtime=LumeRuntime(),
        name="cua-test-macos-vm",
    ) as sb:
        result = await sb.shell.run("sw_vers")
        assert result.success
        assert "macOS" in result.stdout or "Mac" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


# ── 5. Android VM (bare-metal QEMU) ──────────────────────────────────────────


def _has_android_iso() -> bool:
    """Check if an Android-x86 ISO is available in ~/Downloads."""
    from pathlib import Path

    return any(Path.home().glob("Downloads/android-x86*.iso"))


def _get_android_iso() -> str:
    from pathlib import Path

    isos = sorted(Path.home().glob("Downloads/android-x86*.iso"))
    return str(isos[0]) if isos else ""


@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
@pytest.mark.skipif(
    not _has_android_image(),
    reason="Android-x86 qcow2 not available at ~/.cua/cua-sandbox/qemu-storage/android-x86.qcow2",
)
async def test_android_vm_baremetal():
    """Test Android VM via bare-metal QEMU with a pre-built qcow2."""
    from pathlib import Path

    from cua_sandbox.runtime import QEMURuntime

    android_disk = str(Path.home() / ".cua" / "cua-sandbox" / "qemu-storage" / "android-x86.qcow2")
    async with Sandbox.ephemeral(
        Image.from_file(android_disk, os_type="android", kind="vm"),
        local=True,
        runtime=QEMURuntime(
            mode="bare-metal", api_port=18010, vnc_display=10, memory_mb=4096, cpu_count=4
        ),
        name="cua-test-android-vm",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000


@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
@pytest.mark.skipif(not _has_android_iso(), reason="No android-x86*.iso in ~/Downloads")
async def test_android_vm_from_iso():
    """Test Android VM booted from an ISO file via bare-metal QEMU.

    The runtime auto-creates a qcow2 disk and attaches the ISO as CD-ROM.
    Uses QMP transport (no computer-server required).
    """
    from cua_sandbox.runtime import QEMURuntime

    iso_path = _get_android_iso()
    async with Sandbox.ephemeral(
        Image.from_file(iso_path, os_type="android", kind="vm"),
        local=True,
        runtime=QEMURuntime(
            mode="bare-metal",
            api_port=18030,
            vnc_display=30,
            qmp_port=4460,
            memory_mb=2048,
            cpu_count=2,
        ),
        name="cua-test-android-iso3",
    ) as sb:
        # Wait for boot to progress past GRUB (Enter keys are sent during start())
        import asyncio

        await asyncio.sleep(15)

        # With QMP transport, we can take screenshots even without computer-server
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 100

        # Save screenshot for inspection
        with open("/tmp/android-boot-screenshot.png", "wb") as f:
            f.write(screenshot)


# ── 5b. Android VM (QEMU-in-Docker) ──────────────────────────────────────────


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_android_vm_docker():
    """Test Android VM via QEMU inside Docker."""
    from cua_sandbox.runtime import QEMURuntime

    async with Sandbox.ephemeral(
        Image.android("14"),
        local=True,
        runtime=QEMURuntime(mode="docker", api_port=18011, vnc_port=18080, ephemeral=True),
        name="cua-test-android-docker",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000


# ── 6. QEMU bare-metal error on missing binary ───────────────────────────────


def _has_osworld_image() -> bool:
    """Check if the OSWorld Ubuntu qcow2 is available in the image cache."""
    cache = Path.home() / ".cua" / "cua-sandbox" / "image-cache"
    return any(cache.rglob("Ubuntu.qcow2")) if cache.exists() else False


@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
@pytest.mark.skipif(not _has_osworld_image(), reason="OSWorld Ubuntu.qcow2 not in image cache")
async def test_osworld_ubuntu_vm():
    """Test OSWorld Ubuntu VM via bare-metal QEMU with OSWorld transport."""
    from cua_sandbox.runtime import QEMURuntime

    async with Sandbox.ephemeral(
        Image.from_file(
            "https://huggingface.co/datasets/xlangai/ubuntu_osworld/resolve/main/Ubuntu.qcow2.zip",
            os_type="linux",
            agent_type="osworld",
        ),
        local=True,
        runtime=QEMURuntime(
            mode="bare-metal", api_port=18020, vnc_display=20, memory_mb=4096, cpu_count=4
        ),
        name="cua-test-osworld",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"

        w, h = await sb.get_dimensions()
        assert w == 1920 and h == 1080


# ── 7. Android Emulator (SDK) ────────────────────────────────────────────────


def _has_android_sdk() -> bool:
    """Check if Android SDK emulator is available."""
    try:
        from cua_sandbox.runtime.android_emulator import _find_bin, _sdk_path

        sdk = _sdk_path()
        _find_bin(sdk, "emulator")
        _find_bin(sdk, "adb")
        return True
    except (FileNotFoundError, Exception):
        return False


@pytest.mark.skipif(not _has_android_sdk(), reason="Android SDK not available")
async def test_android_emulator():
    """Test Android emulator boots to homescreen."""
    from cua_sandbox.runtime import AndroidEmulatorRuntime

    async with Sandbox.ephemeral(
        Image.android("14"),
        local=True,
        runtime=AndroidEmulatorRuntime(memory_mb=4096, cpu_count=4, adb_port=5559),
        name="cua-test-android-sdk",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 10000


@pytest.mark.skipif(not _has_android_sdk(), reason="Android SDK not available")
async def test_android_emulator_apk_install():
    """Test Android emulator with F-Droid APK installed from URL."""
    from cua_sandbox.runtime import AndroidEmulatorRuntime

    img = Image.android("14").apk_install("https://f-droid.org/F-Droid.apk")
    async with Sandbox.ephemeral(
        img,
        local=True,
        runtime=AndroidEmulatorRuntime(memory_mb=4096, cpu_count=4, adb_port=5561),
        name="cua-test-android-apk",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"

        # Verify F-Droid is installed
        result = await sb._transport.send("shell", command="pm list packages | grep fdroid")
        assert "org.fdroid.fdroid" in result["output"]


# ── 8. Tart (Apple VZ) ────────────────────────────────────────────────────────


def _has_tart() -> bool:
    import shutil

    return shutil.which("tart") is not None


@pytest.mark.skipif(not IS_MACOS, reason="Tart only on macOS")
@pytest.mark.skipif(not _has_tart(), reason="Tart CLI not available")
async def test_tart_ubuntu():
    """Test Ubuntu VM via Tart (Apple VZ)."""
    from cua_sandbox.runtime import TartRuntime

    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/cirruslabs/ubuntu:latest"),
        local=True,
        runtime=TartRuntime(ephemeral=True, display="1024x768"),
        name="cua-test-tart-ubuntu",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000

        result = await sb.shell.run("uname -a")
        assert result.success
        assert "Linux" in result.stdout


@pytest.mark.skipif(not IS_MACOS, reason="Tart only on macOS")
@pytest.mark.skipif(not _has_tart(), reason="Tart CLI not available")
async def test_tart_macos_tahoe():
    """Test macOS Tahoe VM via Tart (Apple VZ)."""
    from cua_sandbox.runtime import TartRuntime

    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/cirruslabs/macos-tahoe-base:latest"),
        local=True,
        runtime=TartRuntime(ephemeral=True),
        name="cua-test-tart-tahoe",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000


@pytest.mark.skipif(not IS_MACOS, reason="Tart only on macOS")
@pytest.mark.skipif(not _has_tart(), reason="Tart CLI not available")
async def test_tart_macos_sequoia_cua():
    """Test CUA macOS Sequoia sparse image via Tart."""
    from cua_sandbox.runtime import TartRuntime

    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/macos-sequoia-cua-sparse:latest-oci-layered"),
        local=True,
        runtime=TartRuntime(ephemeral=True),
        name="cua-test-tart-sequoia",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000


@pytest.mark.skipif(not IS_MACOS or not _has_lume(), reason="Lume only on macOS")
async def test_lume_macos_tahoe_cua():
    """Test CUA macOS Tahoe image via Lume from ghcr.io/trycua/macos-tahoe-cua:latest."""
    from cua_sandbox.runtime import LumeRuntime

    async with Sandbox.ephemeral(
        Image.from_registry("ghcr.io/trycua/macos-tahoe-cua:latest"),
        local=True,
        runtime=LumeRuntime(),
        name="cua-test-lume-tahoe",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000



# ── 9. QEMU bare-metal error on missing binary ───────────────────────────────


def test_qemu_baremetal_missing_binary_error():
    """When QEMU isn't installed, bare-metal runtime should give a helpful error."""
    import shutil

    if shutil.which("qemu-system-x86_64"):
        pytest.skip("QEMU is installed — cannot test missing-binary error")

    with pytest.raises(RuntimeError, match="brew install qemu|not found"):
        from cua_sandbox.runtime.qemu_installer import qemu_bin

        qemu_bin("x86_64")


# ── 10. Auto-runtime (local=True, no explicit runtime) ───────────────────────


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_auto_runtime_linux_container():
    """local=True with no runtime auto-selects DockerRuntime for linux container images."""
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),  # kind="container" by default
        local=True,
        name="cua-test-auto-linux-container",
    ) as sb:
        result = await sb.shell.run("uname -s")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_auto_runtime_linux_vm():
    """local=True with no runtime auto-selects QEMURuntime(docker) for linux vm images."""
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04", kind="vm"),
        local=True,
        name="cua-test-auto-linux-vm",
    ) as sb:
        result = await sb.shell.run("uname -s")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


@pytest.mark.skipif(not IS_MACOS or not _has_lume(), reason="Lume only on macOS")
async def test_auto_runtime_macos():
    """local=True with no runtime auto-selects LumeRuntime for macOS images."""
    async with Sandbox.ephemeral(
        Image.macos("26"),
        local=True,
        name="cua-test-auto-macos",
    ) as sb:
        result = await sb.shell.run("sw_vers")
        assert result.success
        assert "macOS" in result.stdout or "Mac" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


@pytest.mark.skipif(not _has_android_sdk(), reason="Android SDK not available")
async def test_auto_runtime_android():
    """local=True with no runtime auto-selects AndroidEmulatorRuntime for android images."""
    async with Sandbox.ephemeral(
        Image.android("14"),
        local=True,
        name="cua-test-auto-android",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 10000


@pytest.mark.skipif(not IS_WINDOWS, reason="Windows host only")
@pytest.mark.skipif(not _has_qemu(), reason="QEMU not available")
async def test_auto_runtime_windows():
    """local=True with no runtime auto-selects HyperV or QEMU for Windows images."""
    async with Sandbox.ephemeral(
        Image.windows("11"),
        local=True,
        name="cua-test-auto-windows",
    ) as sb:
        result = await sb.shell.run("ver")
        assert result.success or "Windows" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


# ── 11. Cloud sandboxes (local=False) ────────────────────────────────────────


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_cloud_linux_ephemeral():
    """Cloud sandbox: Sandbox.ephemeral with linux image (local=False)."""
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        name="cua-test-cloud-linux",
    ) as sb:
        result = await sb.shell.run("uname -s")
        assert result.success
        assert "Linux" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_cloud_android_ephemeral():
    """Cloud sandbox: Sandbox.ephemeral with android image (local=False)."""
    async with Sandbox.ephemeral(
        Image.android("14"),
        name="cua-test-cloud-android",
    ) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000


# ── 12. Sandbox.create (persistent) ─────────────────────────────────────────


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
async def test_create_persistent_linux():
    """Sandbox.create provisions a persistent sandbox that survives disconnect."""
    from cua_sandbox.runtime import DockerRuntime

    sb = await Sandbox.create(
        Image.linux("ubuntu", "24.04"),
        local=True,
        runtime=DockerRuntime(api_port=18090, vnc_port=18091, ephemeral=True),
        name="cua-test-create-linux",
    )
    try:
        result = await sb.shell.run("echo persistent")
        assert result.success
        assert "persistent" in result.stdout

        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
    finally:
        await sb.disconnect()


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_create_persistent_cloud_linux():
    """Sandbox.create provisions a persistent cloud sandbox."""
    sb = await Sandbox.create(
        Image.linux("ubuntu", "24.04"),
        name="cua-test-create-cloud-linux",
    )
    try:
        result = await sb.shell.run("echo persistent")
        assert result.success
        assert "persistent" in result.stdout
    finally:
        await sb.disconnect()
