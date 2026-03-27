"""Runtime compatibility checks — host/guest OS + arch + hardware acceleration.

Answers two questions for any Image before you try to boot it locally:

1. Is the required runtime installed (or auto-installable) on this host?
2. Does the host OS/arch support hardware acceleration for the guest OS/arch?

Usage::

    from cua_sandbox import Image
    from cua_sandbox.runtime.compat import check_local_support

    support = check_local_support(Image.linux())
    print(support.supported)   # True if Docker is present
    print(support.hw_accel)    # True for containers (native), True on Linux VMs with KVM
    print(support.reason)      # human-readable summary

    # Or via the Image method:
    support = Image.macos().local_support()

In pytest, use the bundled helper::

    from cua_sandbox.runtime.compat import skip_if_unsupported

    async def test_something():
        skip_if_unsupported(Image.macos())
        async with Sandbox.ephemeral(Image.macos(), local=True) as sb:
            ...
"""

from __future__ import annotations

import os
import platform
import shutil
import subprocess
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from cua_sandbox.image import Image


# ---------------------------------------------------------------------------
# Result type
# ---------------------------------------------------------------------------


@dataclass
class RuntimeSupport:
    """Result of a local runtime compatibility check for a given Image.

    Attributes:
        supported:         Image can run locally at all (runtime available or
                           auto-installable, and host/guest OS is compatible).
        hw_accel:          Hardware acceleration is available for the guest on
                           this host (HVF on Apple Silicon, KVM on Linux, etc.).
                           Always True for containers — they run natively.
        runtime_installed: Required runtime binary/daemon is found right now
                           without any installation step.
        auto_installable:  Runtime can be downloaded and installed automatically
                           by the SDK without user intervention (e.g. Android SDK).
        runtime_name:      Name of the runtime that would be used
                           ("docker", "lume", "qemu", "android_emulator", "hyperv").
        host_os:           Detected host OS ("darwin", "linux", "windows").
        host_arch:         Detected host CPU architecture ("arm64", "x86_64").
        guest_os:          Guest OS from the image ("linux", "macos", "windows", "android").
        reason:            Human-readable explanation — always set, describes
                           the limiting factor when supported=False or hw_accel=False.
    """

    supported: bool
    hw_accel: bool
    runtime_installed: bool
    auto_installable: bool
    runtime_name: str
    host_os: str
    host_arch: str
    guest_os: str
    reason: str

    def __str__(self) -> str:
        accel = "hw-accel" if self.hw_accel else "software-only"
        status = "supported" if self.supported else "unsupported"
        installed = (
            "installed"
            if self.runtime_installed
            else ("auto-installable" if self.auto_installable else "not-installed")
        )
        return (
            f"RuntimeSupport({self.guest_os} on {self.host_os}/{self.host_arch}: "
            f"{status}, {accel}, runtime={self.runtime_name} [{installed}])"
        )


# ---------------------------------------------------------------------------
# Internal host probes
# ---------------------------------------------------------------------------


def _host_os() -> str:
    s = platform.system().lower()
    if s == "darwin":
        return "darwin"
    if s == "windows":
        return "windows"
    return "linux"


def _host_arch() -> str:
    m = platform.machine().lower()
    if m in ("arm64", "aarch64"):
        return "arm64"
    return "x86_64"


def _has_docker() -> bool:
    try:
        subprocess.run(
            ["docker", "info"],
            capture_output=True,
            check=True,
            timeout=10,
        )
        return True
    except (subprocess.SubprocessError, FileNotFoundError, OSError):
        return False


def _has_kvm() -> bool:
    from pathlib import Path

    return _host_os() == "linux" and Path("/dev/kvm").exists()


def _has_hvf_for_arm64_guest() -> bool:
    """HVF for ARM64 guests (macOS VMs, ARM Android) — Apple Silicon only."""
    return _host_os() == "darwin" and _host_arch() == "arm64"


def _has_hvf_for_x86_guest() -> bool:
    """HVF for x86_64 guests (Windows, Linux VMs) — Intel Macs only.

    On Apple Silicon, QEMU must use TCG software emulation for x86_64 guests
    because HVF does not support cross-architecture virtualisation.
    On Intel Macs, QEMU can use HVF (-accel hvf) for x86_64 guests.
    """
    return _host_os() == "darwin" and _host_arch() == "x86_64"


def _x86_guest_hw_accel() -> tuple[bool, str]:
    """Return (hw_accel, reason) for an x86_64 guest on the current host."""
    os_ = _host_os()
    arch = _host_arch()
    if os_ == "linux" and _has_kvm():
        return True, "KVM hardware acceleration."
    if os_ == "darwin" and arch == "x86_64":
        return True, "Apple Hypervisor.framework (HVF) via QEMU -accel hvf on Intel Mac."
    if os_ == "windows" and _has_hyperv():
        return True, "Hyper-V hardware acceleration."
    if os_ == "darwin" and arch == "arm64":
        return False, (
            "QEMU cannot use HVF for x86_64 guests on Apple Silicon — TCG software emulation only. "
            "Expect slow performance."
        )
    return False, (
        f"No hardware acceleration for x86_64 guest on {os_}/{arch}. "
        "Enable KVM (Linux) or Hyper-V (Windows) for acceleration."
    )


def _has_lume() -> bool:
    if shutil.which("lume"):
        return True
    local_bin = os.path.expanduser("~/.local/bin/lume")
    return os.path.isfile(local_bin) and os.access(local_bin, os.X_OK)


def _has_android_sdk() -> bool:
    """Return True if the Android emulator binary is already present.

    Uses the same search order as AndroidEmulatorRuntime._sdk_path() so the
    result here matches what the runtime will actually find at boot time.
    """
    import os
    from pathlib import Path

    # 1. Explicit env vars
    for env in ("ANDROID_HOME", "ANDROID_SDK_ROOT"):
        val = os.environ.get(env)
        if val and Path(val).exists():
            sdk = Path(val)
            if (sdk / "emulator" / "emulator").exists():
                return True

    # 2. Common install locations (same list as _sdk_path in android_emulator.py)
    candidates = [
        Path.home() / "Library" / "Android" / "sdk",  # Android Studio macOS
        Path("/opt/android"),  # docker-android
        Path.home() / "Android" / "Sdk",  # Linux
    ]
    for p in candidates:
        if (p / "emulator").exists() and (p / "emulator" / "emulator").exists():
            return True

    # 3. CUA auto-install location (~/.cua/android-sdk) — the fallback _SDK_ROOT
    cua_sdk = Path.home() / ".cua" / "android-sdk"
    return (cua_sdk / "emulator" / "emulator").exists()


def _has_java() -> bool:
    """Return True if a usable JRE is found.

    Mirrors the Homebrew path search in AndroidEmulatorRuntime._java_env()
    so the result is consistent with what the runtime will use at boot.
    """
    from pathlib import Path

    # 1. JAVA_HOME already set
    if shutil.which("java"):
        try:
            r = subprocess.run(["java", "-version"], capture_output=True, timeout=5)
            if r.returncode == 0:
                return True
        except (subprocess.SubprocessError, FileNotFoundError, OSError):
            pass

    # 2. Homebrew OpenJDK — checked in the same order as android_emulator._java_env()
    brew_candidates = [
        Path("/opt/homebrew/opt/openjdk/bin/java"),  # Apple Silicon, latest
        Path("/usr/local/opt/openjdk/bin/java"),  # Intel Mac, latest
    ]
    # Also accept any versioned openjdk (openjdk@17, openjdk@21, etc.)
    for prefix in (Path("/opt/homebrew/opt"), Path("/usr/local/opt")):
        if prefix.exists():
            for d in sorted(prefix.glob("openjdk*")):
                brew_candidates.append(d / "bin" / "java")

    for java_bin in brew_candidates:
        if java_bin.exists() and os.access(java_bin, os.X_OK):
            try:
                r = subprocess.run([str(java_bin), "-version"], capture_output=True, timeout=5)
                if r.returncode == 0:
                    return True
            except (subprocess.SubprocessError, OSError):
                continue

    return False


def _has_hyperv() -> bool:
    try:
        r = subprocess.run(
            ["powershell", "-Command", "Get-Command New-VM -ErrorAction Stop"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return r.returncode == 0
    except (subprocess.SubprocessError, FileNotFoundError, OSError):
        return False


def _has_qemu() -> bool:
    return bool(shutil.which("qemu-system-x86_64") or shutil.which("qemu-system-aarch64"))


# ---------------------------------------------------------------------------
# Main check
# ---------------------------------------------------------------------------


def check_local_support(image: "Image") -> RuntimeSupport:
    """Return a RuntimeSupport describing whether *image* can run locally.

    Args:
        image: The Image to check. Use Image.linux(), Image.macos(), etc.

    Returns:
        RuntimeSupport with .supported, .hw_accel, .reason, and more.
    """
    os_ = _host_os()
    arch = _host_arch()
    guest = image.os_type
    kind = image.kind or "vm"

    # ── Linux container ────────────────────────────────────────────────────
    if guest == "linux" and kind == "container":
        installed = _has_docker()
        return RuntimeSupport(
            supported=installed,
            hw_accel=True,  # containers run natively — no emulation overhead
            runtime_installed=installed,
            auto_installable=False,
            runtime_name="docker",
            host_os=os_,
            host_arch=arch,
            guest_os=guest,
            reason=(
                "Docker is running."
                if installed
                else "Docker is not running or not installed. Install from https://docker.com"
            ),
        )

    # ── Linux VM ──────────────────────────────────────────────────────────
    if guest == "linux" and kind == "vm":
        docker_ok = _has_docker()
        qemu_ok = _has_qemu()
        installed = docker_ok or qemu_ok
        hw, hw_reason = _x86_guest_hw_accel()
        runtime = "docker+qemu" if docker_ok else ("qemu" if qemu_ok else "qemu")
        if not installed:
            reason = (
                "No runtime for Linux VMs found. "
                "Install Docker (preferred) or qemu-system-x86_64."
            )
        else:
            reason = f"Linux VM via QEMU. {hw_reason}"
        return RuntimeSupport(
            supported=installed,
            hw_accel=hw,
            runtime_installed=installed,
            auto_installable=False,
            runtime_name=runtime,
            host_os=os_,
            host_arch=arch,
            guest_os=guest,
            reason=reason,
        )

    # ── macOS VM ──────────────────────────────────────────────────────────
    if guest == "macos":
        # macOS VMs require macOS host + Apple Silicon (HVF via Virtualization.framework)
        if os_ != "darwin":
            return RuntimeSupport(
                supported=False,
                hw_accel=False,
                runtime_installed=False,
                auto_installable=False,
                runtime_name="lume",
                host_os=os_,
                host_arch=arch,
                guest_os=guest,
                reason=f"macOS VMs require a macOS host. Current host OS: {os_}.",
            )
        if arch != "arm64":
            return RuntimeSupport(
                supported=False,
                hw_accel=False,
                runtime_installed=False,
                auto_installable=False,
                runtime_name="lume",
                host_os=os_,
                host_arch=arch,
                guest_os=guest,
                reason=(
                    "macOS VMs require Apple Silicon (ARM64) for Apple Hypervisor.framework. "
                    f"Current host arch: {arch} (Intel Macs cannot run macOS VMs via Lume)."
                ),
            )
        # Apple Silicon macOS — HVF always available
        installed = _has_lume()
        return RuntimeSupport(
            supported=True,  # Lume can be auto-installed via `lume install`
            hw_accel=True,  # HVF always present on Apple Silicon
            runtime_installed=installed,
            auto_installable=True,  # Lume installs via a single curl/brew command
            runtime_name="lume",
            host_os=os_,
            host_arch=arch,
            guest_os=guest,
            reason=(
                "macOS VM with Apple Hypervisor.framework (HVF) acceleration."
                if installed
                else (
                    "Lume CLI not installed but can be auto-installed. "
                    "Run: brew install trycua/tap/lume  or  curl -fsSL https://lume.sh | sh"
                )
            ),
        )

    # ── Windows VM ────────────────────────────────────────────────────────
    if guest == "windows":
        if os_ == "windows" and _has_hyperv():
            return RuntimeSupport(
                supported=True,
                hw_accel=True,
                runtime_installed=True,
                auto_installable=False,
                runtime_name="hyperv",
                host_os=os_,
                host_arch=arch,
                guest_os=guest,
                reason="Windows VM with Hyper-V hardware acceleration.",
            )
        docker_ok = _has_docker()
        qemu_ok = _has_qemu()
        installed = docker_ok or qemu_ok
        hw, hw_reason = _x86_guest_hw_accel()
        runtime = "docker+qemu" if docker_ok else ("qemu" if qemu_ok else "qemu")
        if not installed:
            reason = (
                "No runtime for Windows VMs found. "
                "Install Docker (preferred) or qemu-system-x86_64."
            )
        else:
            reason = f"Windows VM via QEMU. {hw_reason}"
        return RuntimeSupport(
            supported=installed,
            hw_accel=hw,
            runtime_installed=installed,
            auto_installable=False,
            runtime_name=runtime,
            host_os=os_,
            host_arch=arch,
            guest_os=guest,
            reason=reason,
        )

    # ── Android VM ────────────────────────────────────────────────────────
    if guest == "android":
        if os_ == "windows":
            return RuntimeSupport(
                supported=False,
                hw_accel=False,
                runtime_installed=False,
                auto_installable=False,
                runtime_name="android_emulator",
                host_os=os_,
                host_arch=arch,
                guest_os=guest,
                reason=(
                    "Android emulator auto-install is not supported on Windows. "
                    "Install Android Studio manually."
                ),
            )
        sdk_ok = _has_android_sdk()
        java_ok = _has_java()
        # HW accel matrix for Android:
        #   Apple Silicon (arm64): HVF for ARM64 Android system images
        #   Intel Mac (x86_64):    HVF for x86_64 Android system images
        #   Linux x86_64 w/ KVM:   KVM for x86_64 Android system images
        if os_ == "darwin" and arch == "arm64":
            hw = True
            hw_reason = "Apple Hypervisor.framework (HVF) with ARM64 Android system image."
        elif os_ == "darwin" and arch == "x86_64":
            hw = True
            hw_reason = (
                "Apple Hypervisor.framework (HVF) with x86_64 Android system image on Intel Mac."
            )
        elif os_ == "linux" and _has_kvm():
            hw = True
            hw_reason = "KVM with x86_64 Android system image."
        else:
            hw = False
            hw_reason = (
                f"No hardware acceleration for Android on {os_}/{arch}. "
                "Emulator will use software rendering (slow)."
            )

        installed = sdk_ok and java_ok
        # SDK auto-installs on macOS and Linux
        auto = not installed  # if not installed, can we auto-install?

        if not java_ok:
            reason = (
                "Java not found. Install via: brew install openjdk  (macOS) "
                "or  apt install default-jdk  (Linux). " + hw_reason
            )
        elif not sdk_ok:
            reason = "Android SDK not found but will be auto-installed on first boot. " + hw_reason
        else:
            reason = "Android SDK installed. " + hw_reason

        return RuntimeSupport(
            supported=True,  # SDK auto-installs on macOS/Linux if Java is present
            hw_accel=hw,
            runtime_installed=installed,
            auto_installable=auto and java_ok,
            runtime_name="android_emulator",
            host_os=os_,
            host_arch=arch,
            guest_os=guest,
            reason=reason,
        )

    # ── Unknown / registry image with unresolved kind ─────────────────────
    return RuntimeSupport(
        supported=False,
        hw_accel=False,
        runtime_installed=False,
        auto_installable=False,
        runtime_name="unknown",
        host_os=os_,
        host_arch=arch,
        guest_os=guest,
        reason=(
            f"Cannot determine runtime for os_type={guest!r}, kind={kind!r}. "
            "Use Image.linux() / .macos() / .windows() / .android() "
            "or pass runtime= explicitly."
        ),
    )


# ---------------------------------------------------------------------------
# Pytest helper
# ---------------------------------------------------------------------------


def skip_if_unsupported(image: "Image", *, require_hw_accel: bool = False) -> None:
    """Call at the top of a pytest test/fixture to skip when the host can't run *image*.

    Args:
        image:            The Image the test intends to boot with local=True.
        require_hw_accel: If True, also skip when hardware acceleration is unavailable
                          (e.g. skip Android tests that would run in software emulation).

    Example::

        async def test_macos_brew_install():
            skip_if_unsupported(Image.macos())
            async with Sandbox.ephemeral(Image.macos().brew_install("wget"), local=True) as sb:
                assert (await sb.shell.run("wget --version")).success
    """
    import pytest  # imported here so compat.py has no hard pytest dep at import time

    support = check_local_support(image)
    if not support.supported:
        pytest.skip(f"[{support.runtime_name}] {support.reason}")
    if require_hw_accel and not support.hw_accel:
        pytest.skip(
            f"[{support.runtime_name}] hardware acceleration not available: {support.reason}"
        )
