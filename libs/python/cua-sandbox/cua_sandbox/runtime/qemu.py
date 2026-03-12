"""QEMU runtime — bare-metal or Docker-wrapped QEMU VMs.

Two modes:
  QEMURuntime(mode="docker")     — default, uses trycua/cua-qemu-* Docker images
  QEMURuntime(mode="bare-metal") — launches qemu-system-* directly on the host
"""

from __future__ import annotations

import asyncio
import logging
import platform as _plat
import shutil
import subprocess
from pathlib import Path
from typing import Optional

import httpx

from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo
from cua_sandbox.runtime.docker import DockerRuntime, _has_kvm
from cua_sandbox.runtime.images import (
    DEFAULT_API_PORT,
    QEMU_VNC_PORT,
)

logger = logging.getLogger(__name__)

# ── Storage directory for QEMU disk images ──────────────────────────────────

QEMU_STORAGE_ROOT = Path.home() / ".cua" / "cua-sandbox" / "qemu-storage"


class QEMUDockerRuntime(DockerRuntime):
    """QEMU inside Docker — delegates to DockerRuntime with QEMU image tags.

    For Windows QEMU images, automatically:
    - Creates a /storage volume mount for the QEMU disk
    - Sets KVM=N if /dev/kvm is not available
    - Mounts an existing cached disk image if available
    """

    def __init__(
        self,
        *,
        api_port: int = DEFAULT_API_PORT,
        vnc_port: int = QEMU_VNC_PORT,
        ephemeral: bool = True,
        storage_dir: Optional[str | Path] = None,
        memory_mb: int = 8192,
        cpu_count: int = 4,
    ):
        self._storage_dir = Path(storage_dir) if storage_dir else None
        self._memory_mb = memory_mb
        self._cpu_count = cpu_count
        super().__init__(api_port=api_port, vnc_port=vnc_port, ephemeral=ephemeral)

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        # Resolve storage directory
        storage = self._storage_dir or QEMU_STORAGE_ROOT / name
        storage.mkdir(parents=True, exist_ok=True)

        # Volume: host storage dir → /storage in container
        self.volumes = [f"{storage}:/storage"]

        # Environment
        self.environment = {
            "RAM_SIZE": f"{self._memory_mb // 1024}G",
            "CPU_CORES": str(self._cpu_count),
        }

        # KVM handling
        if _has_kvm():
            self.devices = ["/dev/kvm"]
        else:
            self.environment["KVM"] = "N"

        # Platform — QEMU Windows images are linux/amd64
        if image.os_type == "windows":
            self.platform = "linux/amd64"

        # Longer boot timeout for Windows VMs
        boot_timeout = opts.pop("boot_timeout", 300 if image.os_type == "windows" else 120)

        info = await super().start(image, name, **opts)
        return info

    async def is_ready(self, info: RuntimeInfo, timeout: float = 300) -> bool:
        """Wait for the QEMU VM's computer-server to come up.

        Windows VMs take longer to boot (3-5 min), so default timeout is 300s.
        """
        return await super().is_ready(info, timeout=timeout)


class QEMUBaremetalRuntime(Runtime):
    """Bare-metal QEMU — launches qemu-system-* directly on the host.

    Requires:
      - qemu-system-x86_64 (or qemu-system-aarch64) on PATH
      - A disk image (qcow2/vhdx/raw) with computer-server pre-installed,
        provided via Image.from_file('/path/to/disk.qcow2')
    """

    def __init__(
        self,
        *,
        api_port: int = DEFAULT_API_PORT,
        vnc_display: int = 0,
        memory_mb: int = 4096,
        cpu_count: int = 2,
        arch: str = "x86_64",
        extra_args: Optional[list[str]] = None,
    ):
        self.api_port = api_port
        self.vnc_display = vnc_display
        self.memory_mb = memory_mb
        self.cpu_count = cpu_count
        self.arch = arch
        self.extra_args = extra_args or []
        self._processes: dict[str, subprocess.Popen] = {}

    def _qemu_bin(self) -> str:
        from cua_sandbox.runtime.qemu_installer import qemu_bin
        return qemu_bin(self.arch)

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        ephemeral = opts.pop("ephemeral", True)

        # If image has layers or no direct disk path, use the builder to resolve
        if not opts.get("disk_path") and not image._disk_path and image.kind == "vm":
            from cua_sandbox.builder.build import create_session_disk
            disk_path = str(await create_session_disk(image, name))
        elif image._layers and (image._disk_path or opts.get("disk_path")):
            # Has a base disk AND user layers — build user image + session overlay
            from cua_sandbox.builder.build import create_session_disk
            base = Path(opts.get("disk_path") or image._disk_path)
            disk_path = str(await create_session_disk(image, name, base_disk=base))
        else:
            disk_path = opts.get("disk_path") or image._disk_path

        if not disk_path:
            raise ValueError(
                "Bare-metal QEMU requires a disk image path. "
                "Use Image.from_file('/path/to/disk.qcow2') or pass disk_path='...'"
            )

        self._session_disk = Path(disk_path) if ephemeral else None

        memory = opts.get("memory_mb", self.memory_mb)
        cpus = opts.get("cpu_count", self.cpu_count)
        vnc_display = opts.get("vnc_display", self.vnc_display)
        hostfwd_port = opts.get("api_port", self.api_port)
        enable_kvm = opts.get("enable_kvm", True)

        # Detect disk format from extension
        disk_ext = Path(disk_path).suffix.lower()
        disk_fmt = {".qcow2": "qcow2", ".vhdx": "vhdx", ".raw": "raw", ".img": "raw"}.get(disk_ext, "raw")

        # Locate OVMF UEFI firmware for Windows VMs
        qemu_dir = Path(self._qemu_bin()).parent
        ovmf_code = None
        if image.os_type == "windows":
            for candidate in [
                qemu_dir / "share" / "edk2-x86_64-code.fd",
                Path("/usr/share/OVMF/OVMF_CODE.fd"),
                Path("/usr/share/qemu/edk2-x86_64-code.fd"),
            ]:
                if candidate.exists():
                    ovmf_code = candidate
                    break

        # EFI vars — look next to disk or copy template
        efivars = Path(disk_path).parent / "efivars.fd"
        if ovmf_code and not efivars.exists():
            import shutil as _shutil
            vars_template = qemu_dir / "share" / "edk2-i386-vars.fd"
            if vars_template.exists():
                _shutil.copy2(vars_template, efivars)
            else:
                efivars.write_bytes(b"\x00" * (256 * 1024))

        cmd = [
            self._qemu_bin(),
            "-name", name,
            "-machine", "q35,smm=off",
            "-m", str(memory),
            "-smp", str(cpus),
            "-cpu", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt",
        ]

        # UEFI firmware (Windows requires this)
        if ovmf_code:
            cmd += [
                "-drive", f"if=pflash,format=raw,readonly=on,file={ovmf_code}",
                "-drive", f"if=pflash,format=raw,file={efivars}",
            ]

        cmd += [
            "-drive", f"file={disk_path},format={disk_fmt},if=virtio",
            "-netdev", f"user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:{hostfwd_port}-:8000",
            "-device", "virtio-net-pci,netdev=net0,mac=52:55:00:d1:55:01",
            "-vnc", f":{vnc_display}",
        ]

        # -daemonize not supported on Windows
        if _plat.system() != "Windows":
            cmd.append("-daemonize")

        if enable_kvm:
            if _plat.system() != "Windows":
                cmd.append("-enable-kvm")
            elif ovmf_code:
                # WHPX has MMIO bugs with OVMF pflash — use TCG for UEFI VMs
                cmd += ["-accel", "tcg"]
            else:
                cmd += ["-accel", "whpx"]

        cmd.extend(self.extra_args)

        logger.info(f"Starting bare-metal QEMU: {' '.join(cmd)}")
        if _plat.system() == "Windows":
            # On Windows, run in background via Popen
            proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            self._processes[name] = proc
            # Give QEMU a moment to fail fast
            import time
            time.sleep(2)
            if proc.poll() is not None:
                stderr = proc.stderr.read().decode() if proc.stderr else ""
                raise RuntimeError(f"QEMU launch failed (exit {proc.returncode}): {stderr}")
        else:
            result = subprocess.run(cmd, capture_output=True, text=True)
            if result.returncode != 0:
                raise RuntimeError(f"QEMU launch failed: {result.stderr}")

        info = RuntimeInfo(
            host="localhost",
            api_port=hostfwd_port,
            vnc_port=5900 + vnc_display,
            name=name,
        )
        await self.is_ready(info)
        return info

    async def stop(self, name: str) -> None:
        # Try to kill tracked process first
        proc = self._processes.pop(name, None)
        if proc and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        else:
            # Fallback: find by name
            try:
                if shutil.which("pkill"):
                    subprocess.run(["pkill", "-f", f"qemu.*-name {name}"], capture_output=True)
                else:
                    subprocess.run(
                        ["taskkill", "/F", "/FI", f"WINDOWTITLE eq {name}*"],
                        capture_output=True,
                    )
            except Exception as e:
                logger.warning(f"Failed to stop QEMU VM {name}: {e}")

        # Clean up ephemeral session disk
        session_disk = getattr(self, "_session_disk", None)
        if session_disk and session_disk.exists() and "sessions" in str(session_disk):
            try:
                session_disk.unlink()
                logger.info(f"Removed session disk: {session_disk}")
            except OSError:
                pass

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"Bare-metal QEMU VM {info.name} is ready")
                        return True
                except (httpx.ConnectError, httpx.ReadTimeout, httpx.RemoteProtocolError, httpx.ConnectTimeout):
                    pass
                await asyncio.sleep(2)
        raise TimeoutError(f"Bare-metal QEMU VM {info.name} not ready after {timeout}s")


def _win_to_wsl(p: Path | str) -> str:
    """Convert a Windows path to WSL /mnt/... path."""
    s = str(p).replace("\\", "/")
    # C:/foo → /mnt/c/foo
    if len(s) >= 2 and s[1] == ":":
        return f"/mnt/{s[0].lower()}{s[2:]}"
    return s


class QEMUWSL2Runtime(Runtime):
    """QEMU via WSL2 with KVM hardware acceleration.

    Runs qemu-system-x86_64 inside WSL2 where /dev/kvm is available,
    while keeping disk images on the Windows filesystem. Windows paths
    are automatically converted to /mnt/c/... paths for WSL access.

    This is the fastest way to run QEMU on Windows — native KVM speeds
    vs TCG software emulation (bare-metal Windows) or Hyper-V (needs admin).
    """

    def __init__(
        self,
        *,
        api_port: int = DEFAULT_API_PORT,
        vnc_display: int = 0,
        memory_mb: int = 4096,
        cpu_count: int = 4,
        arch: str = "x86_64",
        extra_args: Optional[list[str]] = None,
    ):
        self.api_port = api_port
        self.vnc_display = vnc_display
        self.memory_mb = memory_mb
        self.cpu_count = cpu_count
        self.arch = arch
        self.extra_args = extra_args or []
        self._processes: dict[str, subprocess.Popen] = {}

    @staticmethod
    def _wsl(cmd: str, timeout: float = 30) -> str:
        """Run a command inside WSL2 and return stdout."""
        r = subprocess.run(
            ["wsl", "-e", "bash", "-c", cmd],
            capture_output=True, text=True, timeout=timeout,
        )
        if r.returncode != 0:
            raise RuntimeError(f"WSL command failed: {r.stderr.strip()}")
        return r.stdout.strip()

    @staticmethod
    def available() -> bool:
        """Check if WSL2 + QEMU + KVM are available."""
        try:
            r = subprocess.run(
                ["wsl", "-e", "bash", "-c",
                 "test -e /dev/kvm && which qemu-system-x86_64"],
                capture_output=True, timeout=10,
            )
            return r.returncode == 0
        except (subprocess.SubprocessError, FileNotFoundError):
            return False

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        ephemeral = opts.pop("ephemeral", True)

        # Resolve disk path
        if not opts.get("disk_path") and not image._disk_path and image.kind == "vm":
            from cua_sandbox.builder.build import create_session_disk
            disk_path = str(await create_session_disk(image, name))
        elif image._layers and (image._disk_path or opts.get("disk_path")):
            from cua_sandbox.builder.build import create_session_disk
            base = Path(opts.get("disk_path") or image._disk_path)
            disk_path = str(await create_session_disk(image, name, base_disk=base))
        else:
            disk_path = opts.get("disk_path") or image._disk_path

        if not disk_path:
            raise ValueError(
                "WSL2 QEMU requires a disk image path. "
                "Use Image.from_file('/path/to/disk.qcow2') or pass disk_path='...'"
            )

        self._session_disk = Path(disk_path) if ephemeral else None

        memory = opts.get("memory_mb", self.memory_mb)
        cpus = opts.get("cpu_count", self.cpu_count)
        vnc_display = opts.get("vnc_display", self.vnc_display)
        hostfwd_port = opts.get("api_port", self.api_port)

        # Convert Windows paths to WSL paths
        wsl_disk = _win_to_wsl(disk_path)
        disk_ext = Path(disk_path).suffix.lower()
        disk_fmt = {".qcow2": "qcow2", ".vhdx": "vhdx", ".raw": "raw", ".img": "raw"}.get(disk_ext, "raw")

        # Locate OVMF inside WSL
        ovmf_code = None
        if image.os_type == "windows":
            for candidate in [
                "/usr/share/OVMF/OVMF_CODE_4M.fd",
                "/usr/share/OVMF/OVMF_CODE.fd",
                "/usr/share/qemu/edk2-x86_64-code.fd",
            ]:
                try:
                    self._wsl(f"test -f {candidate}")
                    ovmf_code = candidate
                    break
                except RuntimeError:
                    pass

        # EFI vars — create in same dir as disk (WSL path)
        efivars_win = Path(disk_path).parent / "efivars.fd"
        wsl_efivars = _win_to_wsl(efivars_win)
        if ovmf_code and not efivars_win.exists():
            # Copy OVMF vars template via WSL
            for vars_candidate in [
                "/usr/share/OVMF/OVMF_VARS_4M.fd",
                "/usr/share/OVMF/OVMF_VARS.fd",
                "/usr/share/qemu/edk2-i386-vars.fd",
            ]:
                try:
                    self._wsl(f"test -f {vars_candidate} && cp {vars_candidate} '{wsl_efivars}'")
                    break
                except RuntimeError:
                    continue
            else:
                # Create empty vars file
                efivars_win.write_bytes(b"\x00" * (256 * 1024))

        # Build QEMU command (runs inside WSL)
        parts = [
            f"qemu-system-{self.arch}",
            f"-name {name}",
            "-machine q35,smm=off",
            f"-m {memory}",
            f"-smp {cpus}",
            "-cpu host",
            "-enable-kvm",
        ]

        if ovmf_code:
            parts += [
                f"-drive if=pflash,format=raw,readonly=on,file={ovmf_code}",
                f"-drive if=pflash,format=raw,file={wsl_efivars}",
            ]

        parts += [
            f"-drive file={wsl_disk},format={disk_fmt},if=virtio",
            f"-netdev user,id=net0,hostfwd=tcp:0.0.0.0:{hostfwd_port}-:8000",
            "-device virtio-net-pci,netdev=net0,mac=52:55:00:d1:55:01",
            f"-vnc :{vnc_display}",
            "-daemonize",
        ]

        for arg in self.extra_args:
            parts.append(arg)

        qemu_cmd = " ".join(parts)
        logger.info(f"Starting QEMU in WSL2: {qemu_cmd}")

        # Launch QEMU inside WSL2 (daemonize means it returns immediately)
        result = subprocess.run(
            ["wsl", "-e", "bash", "-c", qemu_cmd],
            capture_output=True, text=True, timeout=30,
        )
        if result.returncode != 0:
            raise RuntimeError(f"QEMU WSL2 launch failed: {result.stderr}")

        info = RuntimeInfo(
            host="localhost",
            api_port=hostfwd_port,
            vnc_port=5900 + vnc_display,
            name=name,
        )
        await self.is_ready(info)
        return info

    async def stop(self, name: str) -> None:
        try:
            subprocess.run(
                ["wsl", "-e", "bash", "-c", f"pkill -f 'qemu.*-name {name}'"],
                capture_output=True, timeout=10,
            )
        except Exception as e:
            logger.warning(f"Failed to stop QEMU VM {name} in WSL2: {e}")

        session_disk = getattr(self, "_session_disk", None)
        if session_disk and session_disk.exists() and "sessions" in str(session_disk):
            try:
                session_disk.unlink()
                logger.info(f"Removed session disk: {session_disk}")
            except OSError:
                pass

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"WSL2 QEMU VM {info.name} is ready")
                        return True
                except (httpx.ConnectError, httpx.ReadTimeout, httpx.RemoteProtocolError, httpx.ConnectTimeout):
                    pass
                await asyncio.sleep(2)
        raise TimeoutError(f"WSL2 QEMU VM {info.name} not ready after {timeout}s")


def QEMURuntime(mode: str = "docker", **kwargs) -> Runtime:
    """Factory that returns the appropriate QEMU runtime.

    Args:
        mode: "docker" (default), "bare-metal", or "wsl2"
    """
    if mode == "wsl2":
        return QEMUWSL2Runtime(**kwargs)
    if mode == "bare-metal":
        return QEMUBaremetalRuntime(**kwargs)
    return QEMUDockerRuntime(**kwargs)
