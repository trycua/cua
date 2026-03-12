"""Hyper-V runtime — Windows VMs built from ISO via unattended install.

Builds a Windows image from scratch using Hyper-V Gen2 VMs (hardware-accelerated),
with the same Autounattend.xml + CUA server install as the QEMU builder.

Layer chain (all VHDX, cached):
  Layer 0 (base):    base-windows-{ver}.vhdx  — Windows + CUA server (built from ISO)
  Layer 1 (user):    user-{hash}.vhdx         — user's .winget_install()/.run() etc
  Layer 2 (session): session-{name}.vhdx      — ephemeral per-sandbox run

Requires Windows Pro/Enterprise/Education with Hyper-V enabled.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import subprocess
from pathlib import Path
from typing import Optional

import httpx

from cua_sandbox.builder.windows_unattend import (
    create_unattend_iso,
    download_windows_iso,
)
from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo
from cua_sandbox.runtime.images import DEFAULT_API_PORT

logger = logging.getLogger(__name__)

CACHE_DIR = Path.home() / ".cua" / "cua-sandbox" / "hyperv"


def _has_hyperv() -> bool:
    try:
        result = subprocess.run(
            ["powershell", "-Command", "Get-Command New-VM -ErrorAction Stop"],
            capture_output=True, text=True,
        )
        return result.returncode == 0
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


def _ps(cmd: str) -> str:
    """Run a PowerShell command and return stdout. Raises on failure."""
    result = subprocess.run(
        ["powershell", "-NoProfile", "-Command", cmd],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(f"PowerShell error: {result.stderr.strip()}")
    return result.stdout.strip()


def _create_diff_vhdx(parent: Path, child: Path) -> Path:
    """Create a differencing VHDX with the given parent."""
    child.parent.mkdir(parents=True, exist_ok=True)
    _ps(f"New-VHD -ParentPath '{parent}' -Path '{child}' -Differencing")
    logger.info(f"Created differencing VHDX: {child} (parent: {parent})")
    return child


class HyperVRuntime(Runtime):
    """Runs Windows VMs via Hyper-V with layered VHDX caching.

    On first use, builds a base Windows image from ISO using unattended
    install (same Autounattend.xml as the QEMU builder). The base image
    includes CUA computer-server pre-installed. This is cached and reused.

    Each sandbox session gets its own ephemeral differencing disk on top.
    """

    def __init__(
        self,
        *,
        api_port: int = DEFAULT_API_PORT,
        switch_name: str = "Default Switch",
        memory_mb: int = 4096,
        cpu_count: int = 4,
        disk_size_gb: int = 64,
        cache_dir: Optional[Path] = None,
        windows_iso: Optional[str] = None,
        product_key: Optional[str] = None,
    ):
        self.api_port = api_port
        self.switch_name = switch_name
        self.memory_mb = memory_mb
        self.cpu_count = cpu_count
        self.disk_size_gb = disk_size_gb
        self.cache_dir = cache_dir or CACHE_DIR
        self.windows_iso = windows_iso
        self.product_key = product_key

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if not _has_hyperv():
            raise RuntimeError(
                "Hyper-V is not available. Enable it via: "
                "Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All"
            )

        memory = opts.get("memory_mb", self.memory_mb)
        cpus = opts.get("cpu_count", self.cpu_count)
        switch = opts.get("switch_name", self.switch_name)

        # Build the layered VHDX chain
        session_disk = await self._resolve_session_disk(image, name)

        # Clean up any existing VM with this name
        self._cleanup_vm(name)

        logger.info(f"Creating Hyper-V VM {name} from {session_disk}")
        _ps(
            f"New-VM -Name '{name}' "
            f"-MemoryStartupBytes {memory * 1024 * 1024} "
            f"-VHDPath '{session_disk}' "
            f"-SwitchName '{switch}' "
            f"-Generation 2"
        )
        _ps(f"Set-VM -Name '{name}' -ProcessorCount {cpus}")
        _ps(f"Set-VMFirmware -VMName '{name}' -EnableSecureBoot Off")
        _ps(f"Start-VM -Name '{name}'")

        ip = await self._wait_for_ip(name)
        info = RuntimeInfo(host=ip, api_port=self.api_port, name=name)
        await self.is_ready(info)
        return info

    async def _resolve_session_disk(self, image: Image, name: str) -> Path:
        """Build the VHDX layer chain and return the session disk path.

        Layer 0: base-windows-{ver}.vhdx (Windows + CUA server, cached)
        Layer 1: user-{hash}.vhdx (user layers applied, cached) — only if image has layers
        Layer 2: session-{name}.vhdx (ephemeral)
        """
        self.cache_dir.mkdir(parents=True, exist_ok=True)

        version = image.version or "11"

        # Layer 0: Base image (Windows + CUA server installed from ISO)
        base = await self._ensure_base_image(version)

        # Layer 1: User layers (cached, if any)
        if image._layers:
            layer_hash = hashlib.sha256(
                json.dumps(list(image._layers), sort_keys=True).encode()
            ).hexdigest()[:16]
            user_layer = self.cache_dir / f"user-{layer_hash}.vhdx"
            if not user_layer.exists():
                user_layer = await self._build_user_layer(image, base, user_layer)
            backing = user_layer
        else:
            backing = base

        # Layer 2: Ephemeral session disk
        sessions_dir = self.cache_dir / "sessions"
        session_disk = sessions_dir / f"{name}.vhdx"
        if session_disk.exists():
            session_disk.unlink()
        _create_diff_vhdx(backing, session_disk)

        return session_disk

    async def _ensure_base_image(self, version: str = "11") -> Path:
        """Build or return cached base Windows image with CUA server installed."""
        base_vhdx = self.cache_dir / f"base-windows-{version}.vhdx"
        if base_vhdx.exists():
            logger.info(f"Using cached base image: {base_vhdx}")
            return base_vhdx

        logger.info("Building base Windows image (first-time setup, ~15-30 min with Hyper-V acceleration)...")

        work_dir = self.cache_dir / "build"
        work_dir.mkdir(parents=True, exist_ok=True)

        # Get Windows ISO
        win_iso = download_windows_iso(version, work_dir, self.windows_iso)

        # Create unattend ISO (no virtio drivers needed, no startup.nsh needed)
        unattend_iso = create_unattend_iso(
            work_dir,
            self.product_key,
            include_virtio_drivers=False,
            include_startup_nsh=False,
        )

        # Create VHDX disk
        build_vhdx = work_dir / "build-disk.vhdx"
        if build_vhdx.exists():
            build_vhdx.unlink()
        _ps(f"New-VHD -Path '{build_vhdx}' -SizeBytes {self.disk_size_gb}GB -Dynamic")

        vm_name = "cua-hyperv-build"
        self._cleanup_vm(vm_name)

        try:
            # Create Gen2 VM
            _ps(
                f"New-VM -Name '{vm_name}' "
                f"-MemoryStartupBytes {self.memory_mb * 1024 * 1024} "
                f"-VHDPath '{build_vhdx}' "
                f"-SwitchName '{self.switch_name}' "
                f"-Generation 2"
            )
            _ps(f"Set-VM -Name '{vm_name}' -ProcessorCount {self.cpu_count}")
            _ps(f"Set-VMFirmware -VMName '{vm_name}' -EnableSecureBoot Off")

            # Attach Windows ISO as DVD
            _ps(f"Add-VMDvdDrive -VMName '{vm_name}' -Path '{win_iso}'")
            # Attach unattend ISO as second DVD
            _ps(f"Add-VMDvdDrive -VMName '{vm_name}' -Path '{unattend_iso}'")

            # Set boot order: DVD first (Windows installer), then hard drive
            _ps(
                f"$dvd = Get-VMDvdDrive -VMName '{vm_name}' | Where-Object {{ $_.Path -eq '{win_iso}' }}; "
                f"Set-VMFirmware -VMName '{vm_name}' -FirstBootDevice $dvd"
            )

            # Start the VM — Windows Setup finds Autounattend.xml automatically
            logger.info("Starting Windows unattended install via Hyper-V...")
            _ps(f"Start-VM -Name '{vm_name}'")

            # Wait for CUA server to come up (Windows install + first login + setup script)
            ip = await self._wait_for_ip(vm_name, timeout=600)
            logger.info(f"Build VM got IP: {ip}")

            info = RuntimeInfo(host=ip, api_port=self.api_port, name=vm_name)
            await self.is_ready(info, timeout=2700)  # 45 min max
            logger.info("CUA computer-server installed and verified!")

            # Shut down cleanly
            _ps(f"Stop-VM -Name '{vm_name}' -Force")
            await asyncio.sleep(5)

        finally:
            # Remove DVD drives and VM definition (keep the disk)
            try:
                _ps(f"Get-VMDvdDrive -VMName '{vm_name}' | Remove-VMDvdDrive")
            except RuntimeError:
                pass
            try:
                _ps(f"Remove-VM -Name '{vm_name}' -Force")
            except RuntimeError:
                pass

        # Move build disk to final cached location
        build_vhdx.rename(base_vhdx)
        logger.info(f"Base image cached: {base_vhdx}")
        return base_vhdx

    async def _build_user_layer(
        self, image: Image, parent: Path, user_layer: Path
    ) -> Path:
        """Apply user Image layers on top of the base image."""
        logger.info(f"Building user layer ({len(image._layers)} layers)...")

        build_disk = user_layer.with_suffix(".build.vhdx")
        if build_disk.exists():
            build_disk.unlink()
        _create_diff_vhdx(parent, build_disk)

        vm_name = "cua-hyperv-build-user"
        self._cleanup_vm(vm_name)

        _ps(
            f"New-VM -Name '{vm_name}' "
            f"-MemoryStartupBytes {self.memory_mb * 1024 * 1024} "
            f"-VHDPath '{build_disk}' "
            f"-SwitchName '{self.switch_name}' "
            f"-Generation 2"
        )
        _ps(f"Set-VM -Name '{vm_name}' -ProcessorCount {self.cpu_count}")
        _ps(f"Set-VMFirmware -VMName '{vm_name}' -EnableSecureBoot Off")
        _ps(f"Start-VM -Name '{vm_name}'")

        try:
            ip = await self._wait_for_ip(vm_name, timeout=300)
            info = RuntimeInfo(host=ip, api_port=self.api_port, name=vm_name)
            await self.is_ready(info, timeout=300)

            from cua_sandbox.builder.executor import LayerExecutor
            executor = LayerExecutor(f"http://{ip}:{self.api_port}")
            await executor.execute_layers(list(image._layers))

            logger.info("User layers applied, shutting down build VM...")
            _ps(f"Stop-VM -Name '{vm_name}' -Force")
            await asyncio.sleep(5)
        finally:
            self._cleanup_vm(vm_name)

        build_disk.rename(user_layer)
        meta = user_layer.with_suffix(".json")
        meta.write_text(json.dumps({
            "os_type": image.os_type,
            "layers": list(image._layers),
            "parent": str(parent),
        }, indent=2))

        logger.info(f"User layer cached: {user_layer}")
        return user_layer

    async def _wait_for_ip(self, name: str, timeout: float = 180) -> str:
        deadline = asyncio.get_event_loop().time() + timeout
        while asyncio.get_event_loop().time() < deadline:
            try:
                ip = _ps(
                    f"(Get-VM -Name '{name}' | Get-VMNetworkAdapter).IPAddresses "
                    f"| Where-Object {{ $_ -match '\\d+\\.\\d+\\.\\d+\\.\\d+' }} "
                    f"| Select-Object -First 1"
                )
                if ip and not ip.startswith("0.0.0.0"):
                    return ip
            except RuntimeError:
                pass
            await asyncio.sleep(3)
        raise TimeoutError(f"Hyper-V VM {name} did not get an IP within {timeout}s")

    def _cleanup_vm(self, name: str) -> None:
        """Stop and remove a VM if it exists."""
        try:
            exists = _ps(
                f"Get-VM -Name '{name}' -ErrorAction SilentlyContinue "
                f"| Select-Object -ExpandProperty Name"
            )
            if exists == name:
                _ps(f"Stop-VM -Name '{name}' -Force -ErrorAction SilentlyContinue")
                _ps(f"Remove-VM -Name '{name}' -Force")
        except RuntimeError:
            pass

    async def stop(self, name: str) -> None:
        """Stop the VM, remove it, and delete the ephemeral session disk."""
        self._cleanup_vm(name)
        session_disk = self.cache_dir / "sessions" / f"{name}.vhdx"
        if session_disk.exists():
            session_disk.unlink()
            logger.info(f"Deleted session disk: {session_disk}")

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"Hyper-V VM {info.name} computer-server is ready")
                        return True
                except (httpx.ConnectError, httpx.ReadTimeout,
                        httpx.RemoteProtocolError, httpx.ConnectTimeout):
                    pass
                await asyncio.sleep(3)
        raise TimeoutError(
            f"Hyper-V VM {info.name} computer-server not ready after {timeout}s"
        )
