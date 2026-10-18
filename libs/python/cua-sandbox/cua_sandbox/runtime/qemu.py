"""QEMU runtimes — VMs through the cua SDK's QEMU backend (cua-vmm).

Every mode boots Linux and Windows guests through the SDK: it finds or
provisions QEMU and UEFI firmware, picks the accelerator (KVM, HVF, WHPX,
TCG), forwards cua-spacesd (3211) plus ``Image.expose()`` ports, and keeps
a QMP socket for agentless control. Disks come from the pinned containerDisk
(``vm:<ref>``, pulled by the SDK) or from the local builder (``disk:<path>``:
``Image.from_file``, user layers, locally built Windows bases).

  QEMURuntime(mode="bare-metal") — QEMU on the host
  QEMURuntime(mode="docker")     — same SDK backend (the trycua/cua-qemu-*
                                   wrapper images ran computer-server and are gone)
  QEMURuntime(mode="wsl2")       — same SDK backend on Windows hosts

Kept on the legacy Python launcher (they need no spacesd): Android-x86
guests, OSWorld images, ISO installs, QMP-only transports and custom
``extra_args``. ``CUA_SANDBOX_LEGACY_QEMU=1`` forces the legacy launcher.
"""

from __future__ import annotations

import asyncio
import json as _json
import logging
import platform as _plat
import re
import shutil
import subprocess
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, Optional

from cua_sandbox._paths import cua_home

if TYPE_CHECKING:
    from cua_sandbox.image import Image

import httpx
from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo
from cua_sandbox.runtime.images import DEFAULT_API_PORT, SPACESD_PORT
from cua_sandbox.runtime.native import NativeRuntime, is_native_state

QEMU_VNC_PORT = 8006

logger = logging.getLogger(__name__)

# ── Storage directory for QEMU disk images ──────────────────────────────────

QEMU_STORAGE_ROOT = cua_home() / "cua-sandbox" / "qemu-storage"


async def _qmp_command(
    host: str, port: int, command: str, arguments: Optional[dict] = None
) -> dict:
    """Send a single QMP command and return the response."""
    reader, writer = await asyncio.open_connection(host, port)
    try:
        # Read greeting
        await asyncio.wait_for(reader.readline(), timeout=5)
        # Negotiate capabilities
        writer.write(b'{"execute":"qmp_capabilities"}\n')
        await writer.drain()
        await asyncio.wait_for(reader.readline(), timeout=5)
        # Send command
        msg: dict = {"execute": command}
        if arguments:
            msg["arguments"] = arguments
        writer.write((_json.dumps(msg) + "\n").encode())
        await writer.drain()
        raw = await asyncio.wait_for(reader.readline(), timeout=30)
        return _json.loads(raw)
    finally:
        writer.close()
        try:
            await writer.wait_closed()
        except Exception:
            pass


def _legacy_forced() -> bool:
    import os

    return os.environ.get("CUA_SANDBOX_LEGACY_QEMU", "").lower() in ("1", "true", "yes")


class NativeQEMURuntime(NativeRuntime):
    """Linux/Windows VMs on the SDK's QEMU backend."""

    runtime_type = "qemu"
    env_ready_timeout = 600.0

    def __init__(self, **kwargs) -> None:
        super().__init__(**kwargs)
        self._session_disks: dict[str, Path] = {}

    async def _image_ref(self, image: Image, name: str, **opts) -> str:
        from cua_sandbox.builder.build import create_session_disk, has_build_work
        from cua_sandbox.image import cloud_registry_image

        disk = opts.get("disk_path") or image._disk_path
        ref = cloud_registry_image(image)
        if disk and not has_build_work(image):
            return f"disk:{disk}"
        if ref is not None and not disk and not has_build_work(image):
            # The SDK pulls the pinned containerDisk (the disk Fleet boots).
            return f"vm:{ref}"
        session = await create_session_disk(image, name, base_disk=Path(disk) if disk else None)
        if "sessions" in str(session):
            self._session_disks[name] = Path(session)
        return f"disk:{session}"

    async def delete(self, name: str) -> None:
        try:
            await super().delete(name)
        finally:
            session = self._session_disks.pop(name, None)
            if session is not None and session.exists():
                try:
                    session.unlink()
                except OSError:
                    pass


def _native_eligible(
    image: Image, opts: dict, *, use_qmp: bool = False, extra: bool = False
) -> bool:
    disk = opts.get("disk_path") or image._disk_path
    return (
        not _legacy_forced()
        and image.os_type in ("linux", "windows")
        and image._agent_type != "osworld"
        and not use_qmp
        and not extra
        and not (disk and Path(disk).suffix.lower() == ".iso")
    )


class QEMUDockerRuntime(NativeQEMURuntime):
    """``QEMURuntime(mode="docker")``: kept for compatibility.

    The trycua/cua-qemu-* wrapper images drove their guest through
    computer-server, which is gone; Linux and Windows guests now boot on the
    SDK's QEMU backend (the SDK provisions QEMU itself). Android guests use
    :class:`~cua_sandbox.runtime.AndroidEmulatorRuntime` or bare-metal QEMU.
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
        super().__init__(ephemeral=ephemeral, cpus=cpu_count, memory_mb=memory_mb)
        self.api_port = api_port
        self.vnc_port = vnc_port
        self._storage_dir = Path(storage_dir) if storage_dir else None

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if image.os_type == "android":
            raise NotImplementedError(
                "Docker-wrapped QEMU Android images ran computer-server and were removed; "
                "use AndroidEmulatorRuntime() or QEMURuntime(mode='bare-metal') with an "
                "Android-x86 disk"
            )
        opts.pop("boot_timeout", None)
        return await super().start(image, name, **opts)


# UEFI firmware for Windows guests, as (code, vars-template) pairs. The two halves
# are sized to match each other, so they are always taken from the same entry — a
# 4 MB OVMF build paired with a 2 MB varstore leaves the guest unable to boot.
# Paths are relative to the bundled QEMU directory, or absolute for system installs.
_UEFI_FIRMWARE_CANDIDATES: list[tuple[str, str]] = [
    ("share/edk2-x86_64-code.fd", "share/edk2-i386-vars.fd"),
    ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
    ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd"),
    ("/usr/share/qemu/edk2-x86_64-code.fd", "/usr/share/qemu/edk2-i386-vars.fd"),
]


def _netdev(forwards: str, *, restrict: bool = False) -> str:
    """The ``-netdev`` value: user-mode NAT with ``forwards`` (``hostfwd=...``).

    The guest has outbound network unless ``restrict`` (``network="none"``),
    which adds ``restrict=on``: slirp then drops guest-initiated traffic but
    still serves the host forwards the SDK talks to the guest through.
    """
    return f"user,id=net0,{'restrict=on,' if restrict else ''}{forwards}"


def _find_free_vnc_display(start: int = 0, span: int = 64) -> int:
    """Return a VNC display number whose TCP port (5900+N) is free.

    QEMU exits with "Failed to find an available port" when the display is taken,
    so a fixed default makes the second concurrent local sandbox on a host die.
    """
    import socket

    for display in range(start, start + span):
        # No SO_REUSEADDR here on purpose: it would let this probe bind a port a
        # running VM already listens on, which is exactly what we are testing for.
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            try:
                s.bind(("", 5900 + display))
            except OSError:
                continue
            return display
    return start


def _locate_uefi_firmware(qemu_dir: Path) -> tuple[Optional[Path], Optional[Path]]:
    """Return the first (OVMF code, matching vars template) pair present on this host."""
    for code_name, vars_name in _UEFI_FIRMWARE_CANDIDATES:
        code = Path(code_name) if code_name.startswith("/") else qemu_dir / code_name
        if not code.exists():
            continue
        template = Path(vars_name) if vars_name.startswith("/") else qemu_dir / vars_name
        return code, template if template.exists() else None
    return None, None


def _locate_uefi_firmware_in(exists: Callable[[str], bool]) -> tuple[Optional[str], Optional[str]]:
    """Same lookup as :func:`_locate_uefi_firmware`, for a guest filesystem like WSL.

    Only the absolute candidates apply — a WSL distro has its own /usr/share, and the
    Windows-side bundled QEMU directory is not on its path.
    """
    for code_name, vars_name in _UEFI_FIRMWARE_CANDIDATES:
        if not code_name.startswith("/") or not exists(code_name):
            continue
        return code_name, vars_name if exists(vars_name) else None
    return None, None


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
        qmp_port: int = 4444,
        use_qmp_transport: bool = False,
        extra_args: Optional[list[str]] = None,
        use_sdk: Optional[bool] = None,
    ):
        """``use_sdk=False`` keeps the legacy in-process launcher, which boots
        the given disk in place (the image builder needs that: the SDK always
        puts an instance overlay on top of the disk)."""
        self.api_port = api_port
        self.vnc_display = vnc_display
        self.memory_mb = memory_mb
        self.cpu_count = cpu_count
        self.arch = arch
        self.qmp_port = qmp_port
        self.use_qmp_transport = use_qmp_transport
        self.extra_args = extra_args or []
        self.use_sdk = use_sdk
        self._processes: dict[str, subprocess.Popen] = {}

    def _qemu_bin(self) -> str:
        from cua_sandbox.runtime.qemu_installer import qemu_bin

        return qemu_bin(self.arch)

    @staticmethod
    def _create_disk_for_iso(name: str, size_gb: int = 32) -> Path:
        """Create a qcow2 disk image for ISO-based installations.

        The disk is stored alongside other QEMU storage and reused across sessions.
        """
        disk_dir = QEMU_STORAGE_ROOT / name
        disk_dir.mkdir(parents=True, exist_ok=True)
        disk_path = disk_dir / "disk.qcow2"
        if not disk_path.exists():
            result = subprocess.run(
                ["qemu-img", "create", "-f", "qcow2", str(disk_path), f"{size_gb}G"],
                capture_output=True,
                text=True,
            )
            if result.returncode != 0:
                raise RuntimeError(f"qemu-img create failed: {result.stderr}")
        return disk_path

    def _native_runtime(self) -> NativeQEMURuntime:
        native = getattr(self, "_native", None)
        if native is None:
            native = NativeQEMURuntime(
                cpus=self.cpu_count,
                memory_mb=self.memory_mb,
                server_port=getattr(self, "server_port", None),
            )
            self._native = native
        return native

    def _is_native(self, name: str) -> bool:
        from cua_sandbox import sandbox_state

        if name in getattr(self, "_native_names", set()):
            return True
        return is_native_state(sandbox_state.load(name))

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        ephemeral = opts.pop("ephemeral", True)

        if self.use_sdk is not False and _native_eligible(
            image, opts, use_qmp=self.use_qmp_transport, extra=bool(self.extra_args)
        ):
            native = self._native_runtime()
            native.ephemeral = ephemeral
            info = await native.start(image, name, ephemeral=ephemeral, **opts)
            self._native_names = getattr(self, "_native_names", set()) | {name}
            return info

        # Guest egress is on by default (slirp NAT, like a Docker container);
        # network="none" adds restrict=on, which keeps the loopback forwards.
        restrict = opts.pop("network", None) == "none"

        from cua_sandbox.builder.build import create_session_disk, has_build_work

        # If image has build work or no direct disk path, use the builder to resolve
        if not opts.get("disk_path") and not image._disk_path and image.kind == "vm":
            disk_path = str(await create_session_disk(image, name))
        elif has_build_work(image) and (image._disk_path or opts.get("disk_path")):
            # Has a base disk AND user layers/env/files — build user image + session overlay
            base = Path(opts.get("disk_path") or image._disk_path)
            disk_path = str(await create_session_disk(image, name, base_disk=base))
        else:
            disk_path = opts.get("disk_path") or image._disk_path

        if not disk_path:
            raise ValueError(
                "Bare-metal QEMU requires a disk image path. "
                "Use Image.from_file('/path/to/disk.qcow2') or pass disk_path='...'"
            )

        # Handle ISO files — create a qcow2 disk and boot from ISO as CD-ROM
        self._iso_path: Optional[str] = None
        if Path(disk_path).suffix.lower() == ".iso":
            self._iso_path = disk_path
            disk_path = str(self._create_disk_for_iso(name, opts.get("disk_size_gb", 32)))
            logger.info(f"Created qcow2 disk for ISO install: {disk_path}")

        self._session_disk = Path(disk_path) if ephemeral else None

        from cua_sandbox.runtime.docker import _find_free_port

        memory = opts.get("memory_mb", self.memory_mb)
        cpus = opts.get("cpu_count", self.cpu_count)
        # VNC and QMP need a free port each, the same way the API port does —
        # the class defaults are only a starting point. Two concurrent local
        # sandboxes would otherwise both claim :0 and 4444 and the second dies.
        vnc_display = _find_free_vnc_display(opts.get("vnc_display", self.vnc_display))
        hostfwd_port = opts.get("api_port") or _find_free_port(self.api_port)
        qmp_port = opts.get("qmp_port") or _find_free_port(self.qmp_port)
        enable_kvm = opts.get("enable_kvm", True)

        # Guest agent port: the OSWorld Flask server, else cua-spacesd.
        guest_port = 5000 if image._agent_type == "osworld" else SPACESD_PORT

        # Image.expose() ports. Without these the port is silently unreachable
        # from the host: the guest listens, nothing forwards, and nothing errors.
        exposed_ports: dict = {}
        extra_hostfwd = ""
        for exposed in dict.fromkeys(image._ports):
            if exposed == guest_port:
                # already forwarded as the guest agent port
                exposed_ports[exposed] = hostfwd_port
                continue
            host_port = _find_free_port(exposed)
            while host_port in exposed_ports.values() or host_port in (hostfwd_port, qmp_port):
                host_port = _find_free_port(host_port + 1)
            exposed_ports[exposed] = host_port
            extra_hostfwd += f",hostfwd=tcp:127.0.0.1:{host_port}-:{exposed}"
        if exposed_ports:
            logger.info(f"Forwarding exposed ports (guest -> host): {exposed_ports}")

        # Detect disk format from extension
        disk_ext = Path(disk_path).suffix.lower()
        disk_fmt = {".qcow2": "qcow2", ".vhdx": "vhdx", ".raw": "raw", ".img": "raw"}.get(
            disk_ext, "raw"
        )

        # Locate OVMF UEFI firmware for Windows VMs
        qemu_dir = Path(self._qemu_bin()).parent
        ovmf_code, vars_template = (
            _locate_uefi_firmware(qemu_dir) if image.os_type == "windows" else (None, None)
        )

        # EFI vars — one pflash file per VM, named after its disk. A single shared
        # sessions/efivars.fd would be mapped writable into every concurrent UEFI
        # guest at once, so they would clobber each other's boot variables.
        efivars = Path(disk_path).with_suffix(".efivars.fd")
        if ovmf_code and not efivars.exists():
            import shutil as _shutil

            if vars_template is not None:
                _shutil.copy2(vars_template, efivars)
            else:
                efivars.write_bytes(b"\x00" * (256 * 1024))

        # Build QEMU command — Android gets different machine/device config
        is_android = image.os_type == "android"

        if is_android:
            cmd = self._build_android_cmd(
                name,
                disk_path,
                disk_fmt,
                memory,
                cpus,
                hostfwd_port,
                vnc_display,
                enable_kvm,
                qmp_port,
                restrict=restrict,
            )
        else:
            cmd = [
                self._qemu_bin(),
                "-name",
                name,
                "-machine",
                "q35,smm=off",
                "-m",
                str(memory),
                "-smp",
                str(cpus),
                "-cpu",
                "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt",
            ]

            # UEFI firmware (Windows requires this)
            if ovmf_code:
                cmd += [
                    "-drive",
                    f"if=pflash,format=raw,readonly=on,file={ovmf_code}",
                    "-drive",
                    f"if=pflash,format=raw,file={efivars}",
                ]

            cmd += [
                "-drive",
                f"file={disk_path},format={disk_fmt},if=virtio",
                "-netdev",
                _netdev(
                    f"hostfwd=tcp:127.0.0.1:{hostfwd_port}-:{guest_port}{extra_hostfwd}",
                    restrict=restrict,
                ),
                "-device",
                "virtio-net-pci,netdev=net0,mac=52:55:00:d1:55:01",
                "-vnc",
                f":{vnc_display}",
            ]

            # -daemonize not supported on Windows
            if _plat.system() != "Windows":
                cmd.append("-daemonize")

            if enable_kvm:
                if _plat.system() == "Darwin":
                    host_arm = _plat.machine() in ("arm64", "aarch64")
                    guest_x86 = self.arch == "x86_64"
                    if host_arm and guest_x86:
                        # Apple Silicon can't HVF-accelerate x86_64 guests
                        cmd += ["-accel", "tcg"]
                    else:
                        cmd += ["-accel", "hvf"]
                elif _plat.system() != "Windows":
                    cmd.append("-enable-kvm")
                elif ovmf_code:
                    # WHPX has MMIO bugs with OVMF pflash — use TCG for UEFI VMs
                    cmd += ["-accel", "tcg"]
                else:
                    cmd += ["-accel", "whpx"]

            # QMP socket — always enabled for VM management (suspend/resume)
            cmd += ["-qmp", f"tcp:127.0.0.1:{qmp_port},server,nowait"]

        # Attach ISO as CD-ROM if booting from an ISO file
        if self._iso_path:
            cmd += ["-cdrom", self._iso_path, "-boot", "d"]

        cmd.extend(self.extra_args)

        logger.info(f"Starting bare-metal QEMU: {' '.join(cmd)}")
        if "-daemonize" in cmd:
            # daemonize mode: QEMU forks to background and parent exits immediately
            result = subprocess.run(cmd, capture_output=True, text=True)
            if result.returncode != 0:
                raise RuntimeError(f"QEMU launch failed: {result.stderr}")
        else:
            # Popen mode: keep reference to the process (Windows, or when -daemonize removed)
            proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            self._processes[name] = proc
            import time

            time.sleep(2)
            if proc.poll() is not None:
                stderr = proc.stderr.read().decode() if proc.stderr else ""
                raise RuntimeError(f"QEMU launch failed (exit {proc.returncode}): {stderr}")

        use_qmp = is_android or self.use_qmp_transport
        info = RuntimeInfo(
            host="localhost",
            api_port=hostfwd_port,
            exposed_ports=exposed_ports or None,
            vnc_port=5900 + vnc_display,
            name=name,
            qmp_port=qmp_port if use_qmp else None,
            environment=image.os_type if use_qmp else None,
            agent_type=image._agent_type,
            guest_server_port=guest_port if not is_android else 8000,
        )

        # For ISO boots, send Enter key via QMP to skip GRUB countdown
        if self._iso_path and use_qmp:
            await self._send_boot_key(info)

        # Windows and Android need much longer to boot (3–10 min)
        boot_timeout = 600 if image.os_type in ("windows", "android") else 120
        await self.is_ready(info, timeout=boot_timeout)

        if not ephemeral:
            from cua_sandbox import sandbox_state

            sandbox_state.save(
                name,
                runtime_type="qemu-baremetal",
                image=image.to_dict(),
                host="localhost",
                api_port=hostfwd_port,
                exposed_ports=exposed_ports or None,
                vnc_port=5900 + vnc_display,
                qmp_port=qmp_port,
                disk_path=str(disk_path) if disk_path else None,
                os_type=image.os_type,
                vnc_display=vnc_display,
                memory_mb=memory,
                cpu_count=cpus,
                arch=self.arch,
                network="none" if restrict else "default",
                status="running",
            )

        return info

    async def _send_boot_key(self, info: RuntimeInfo) -> None:
        """Send Enter key via QMP repeatedly to skip GRUB/bootloader countdown.

        GRUB may not be ready immediately after QEMU starts (especially with TCG),
        so we send Enter multiple times over several seconds to ensure it lands.
        """

        for attempt in range(15):
            try:
                reader, writer = await asyncio.open_connection(info.host, info.qmp_port)
                await asyncio.wait_for(reader.readline(), timeout=3)
                writer.write(b'{"execute":"qmp_capabilities"}\n')
                await writer.drain()
                await asyncio.wait_for(reader.readline(), timeout=3)
                # Send Enter key several times with delays to catch GRUB at the right moment
                for i in range(5):
                    writer.write(
                        b'{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"ret"}]}}\n'
                    )
                    await writer.drain()
                    await asyncio.wait_for(reader.readline(), timeout=3)
                    await asyncio.sleep(2)
                writer.close()
                await writer.wait_closed()
                logger.info(f"Sent boot Enter keys to {info.name}")
                return
            except (ConnectionRefusedError, OSError, asyncio.TimeoutError):
                await asyncio.sleep(2)
        logger.warning(f"Could not send boot keys to {info.name}")

    def _build_android_cmd(
        self,
        name: str,
        disk_path: str,
        disk_fmt: str,
        memory: int,
        cpus: int,
        hostfwd_port: int,
        vnc_display: int,
        enable_kvm: bool,
        qmp_port: Optional[int] = None,
        *,
        restrict: bool = False,
    ) -> list[str]:
        """Build QEMU command for Android x86_64 VM.

        Android-x86 boots from a disk image with GRUB, uses virtio for
        disk/net, and needs a GPU (virtio-gpu or std) for the display.
        Port-forwards host:hostfwd_port -> guest:8000 for the computer-server
        API, and host:5555 -> guest:5555 for ADB.
        """
        cmd = [
            self._qemu_bin(),
            "-name",
            name,
            "-machine",
            "q35,smm=off",
            "-m",
            str(memory),
            "-smp",
            str(cpus),
        ]

        # CPU config — use HVF on macOS (aarch64 only), KVM on Linux, TCG fallback
        if _plat.system() == "Darwin":
            # HVF only works when host and guest arch match.
            # Apple Silicon (arm64) can't HVF-accelerate x86_64 guests.
            host_arm = _plat.machine() in ("arm64", "aarch64")
            guest_x86 = self.arch == "x86_64"
            if enable_kvm and not (host_arm and guest_x86):
                cmd += ["-cpu", "host", "-accel", "hvf"]
            else:
                cmd += ["-cpu", "max", "-accel", "tcg"]
        elif enable_kvm and _plat.system() != "Windows":
            cmd += ["-cpu", "host", "-enable-kvm"]
        else:
            cmd += ["-cpu", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt"]

        # Disk — Android-x86 image
        cmd += ["-drive", f"file={disk_path},format={disk_fmt},if=virtio"]

        # Networking — forward API (8000); ADB forwarded on api_port+1
        adb_port = hostfwd_port + 1
        cmd += [
            "-netdev",
            _netdev(
                f"hostfwd=tcp:127.0.0.1:{hostfwd_port}-:8000,"
                f"hostfwd=tcp:127.0.0.1:{adb_port}-:5555",
                restrict=restrict,
            ),
            "-device",
            "virtio-net-pci,netdev=net0",
        ]

        # Display — std VGA for broad compatibility (virtio-gpu hangs on Apple Silicon TCG)
        cmd += [
            "-vga",
            "std",
            "-vnc",
            f":{vnc_display}",
            "-display",
            "none",
        ]

        # USB tablet for absolute pointer (touch input)
        cmd += ["-usb", "-device", "usb-tablet"]

        # QMP socket for direct VM control (mouse/keyboard/screenshot without guest agent)
        cmd += ["-qmp", f"tcp:127.0.0.1:{qmp_port or self.qmp_port},server,nowait"]

        # Daemonize on Unix
        if _plat.system() != "Windows":
            cmd.append("-daemonize")

        return cmd

    async def stop(self, name: str) -> None:
        if self._is_native(name):
            await self._native_runtime().delete(name)
            return
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
                    # Match both standard QEMU VMs (-name {name}) and Android emulators (-avd {name})
                    subprocess.run(["pkill", "-f", f"qemu.*-name {name}"], capture_output=True)
                    subprocess.run(["pkill", "-f", f"qemu.*-avd {name}"], capture_output=True)
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

        from cua_sandbox import sandbox_state

        sandbox_state.delete(name)

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        if info.native is not None:
            return await self._native_runtime().is_ready(info, timeout)
        if info.qmp_port and not info.agent_type:
            return await self._is_ready_qmp(info, timeout)
        if info.agent_type != "osworld":
            # Daemon-agnostic: the QEMU process is up; the interfaces wait for
            # cua-spacesd themselves (and fall back to QMP/VNC without it).
            return True
        # OSWorld images serve their Flask agent; wait for its screenshot route.
        url = f"http://{info.host}:{info.api_port}/screenshot"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=10) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"Bare-metal QEMU VM {info.name} is ready")
                        return True
                except (
                    httpx.ConnectError,
                    httpx.ReadTimeout,
                    httpx.ReadError,
                    httpx.RemoteProtocolError,
                    httpx.ConnectTimeout,
                ):
                    pass
                await asyncio.sleep(3)
        raise TimeoutError(f"Bare-metal QEMU VM {info.name} not ready after {timeout}s")

    async def suspend(self, name: str) -> None:
        """Save VM state via QMP savevm then quit QEMU."""
        from cua_sandbox import sandbox_state

        if self._is_native(name):
            await self._native_runtime().suspend(name)
            return
        state = sandbox_state.load(name)
        qmp_port = state["qmp_port"] if state else self.qmp_port
        try:
            await _qmp_command("localhost", qmp_port, "stop")
            await _qmp_command("localhost", qmp_port, "savevm", {"name": "cua-snapshot"})
            await _qmp_command("localhost", qmp_port, "quit")
        except Exception as e:
            logger.warning(f"QMP savevm failed for {name}: {e}")
            raise
        sandbox_state.update(name, status="suspended")

    async def resume(self, image: "Image", name: str, **opts) -> RuntimeInfo:
        """Relaunch QEMU with -loadvm to restore saved state."""
        from cua_sandbox import sandbox_state

        if self._is_native(name):
            return await self._native_runtime().resume(image, name, **opts)
        state = sandbox_state.load(name)
        if state is None:
            raise ValueError(f"No state file found for sandbox '{name}'. Cannot resume.")

        # Reconstruct runtime params from state
        disk_path = state.get("disk_path")
        if not disk_path:
            raise ValueError(
                f"State for '{name}' has no disk_path — cannot resume bare-metal QEMU."
            )

        api_port = state.get("api_port", self.api_port)
        vnc_display = state.get("vnc_display", self.vnc_display)
        memory = state.get("memory_mb", self.memory_mb)
        cpus = state.get("cpu_count", self.cpu_count)
        qmp_port = state.get("qmp_port", self.qmp_port)

        # Rebuild minimal QEMU command with -loadvm
        cmd = [
            self._qemu_bin(),
            "-name",
            name,
            "-machine",
            "q35,smm=off",
            "-m",
            str(memory),
            "-smp",
            str(cpus),
            "-cpu",
            "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt",
        ]

        import platform as _platform

        disk_ext = Path(disk_path).suffix.lower()
        disk_fmt = {".qcow2": "qcow2", ".vhdx": "vhdx", ".raw": "raw", ".img": "raw"}.get(
            disk_ext, "raw"
        )
        guest_port = SPACESD_PORT

        cmd += [
            "-drive",
            f"file={disk_path},format={disk_fmt},if=virtio",
            "-netdev",
            _netdev(
                f"hostfwd=tcp:127.0.0.1:{api_port}-:{guest_port}",
                restrict=state.get("network") == "none",
            ),
            "-device",
            "virtio-net-pci,netdev=net0,mac=52:55:00:d1:55:01",
            "-vnc",
            f":{vnc_display}",
            "-qmp",
            f"tcp:127.0.0.1:{qmp_port},server,nowait",
            "-loadvm",
            "cua-snapshot",
        ]

        if _platform.system() != "Windows":
            cmd.append("-daemonize")

        # Acceleration
        if _platform.system() == "Darwin":
            host_arm = _platform.machine() in ("arm64", "aarch64")
            if host_arm and self.arch == "x86_64":
                cmd += ["-accel", "tcg"]
            else:
                cmd += ["-accel", "hvf"]
        elif _platform.system() != "Windows":
            cmd.append("-enable-kvm")

        logger.info(f"Resuming bare-metal QEMU from snapshot: {' '.join(cmd)}")
        if "-daemonize" in cmd:
            result = subprocess.run(cmd, capture_output=True, text=True)
            if result.returncode != 0:
                raise RuntimeError(f"QEMU resume failed: {result.stderr}")
        else:
            proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            self._processes[name] = proc
            import time

            time.sleep(2)
            if proc.poll() is not None:
                stderr = proc.stderr.read().decode() if proc.stderr else ""
                raise RuntimeError(f"QEMU resume failed (exit {proc.returncode}): {stderr}")

        info = RuntimeInfo(
            host="localhost",
            api_port=api_port,
            vnc_port=5900 + vnc_display,
            name=name,
        )
        await self.is_ready(info)
        sandbox_state.update(name, status="running")
        return info

    async def list(self) -> list[dict]:
        """List known bare-metal QEMU sandboxes from state files, checking if alive."""
        from cua_sandbox import sandbox_state

        states = [s for s in sandbox_state.list_all() if s.get("runtime_type") == "qemu-baremetal"]
        result = []
        for s in states:
            name = s["name"]
            status = s.get("status", "unknown")
            # Verify QEMU process is still running (standard VMs use -name, Android uses -avd)
            if status == "running":
                try:
                    alive = (
                        subprocess.run(
                            ["pgrep", "-f", f"qemu.*-name {name}"], capture_output=True
                        ).returncode
                        == 0
                        or subprocess.run(
                            ["pgrep", "-f", f"qemu.*-avd {name}"], capture_output=True
                        ).returncode
                        == 0
                    )
                    if not alive:
                        status = "stopped"
                        sandbox_state.update(name, status="stopped")
                except FileNotFoundError:
                    pass
            result.append(
                {
                    "name": name,
                    "status": status,
                    "runtime_type": "qemu-baremetal",
                    "os_type": s.get("os_type"),
                    "host": s.get("host"),
                    "api_port": s.get("api_port"),
                }
            )
        return result

    async def _is_ready_qmp(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        """Wait until QMP socket is responsive (for VMs without computer-server)."""
        import json as _json

        deadline = asyncio.get_event_loop().time() + timeout
        while asyncio.get_event_loop().time() < deadline:
            try:
                reader, writer = await asyncio.open_connection(info.host, info.qmp_port)
                # Read QMP greeting
                greeting = await asyncio.wait_for(reader.readline(), timeout=3)
                if greeting:
                    data = _json.loads(greeting)
                    if "QMP" in data:
                        writer.close()
                        await writer.wait_closed()
                        logger.info(f"Bare-metal QEMU VM {info.name} QMP ready")
                        return True
                writer.close()
                await writer.wait_closed()
            except (ConnectionRefusedError, OSError, asyncio.TimeoutError):
                pass
            await asyncio.sleep(2)
        raise TimeoutError(f"Bare-metal QEMU VM {info.name} QMP not ready after {timeout}s")


# A drive-letter path such as C:\\Users\\... or C:/Users/..., which QEMU running
# inside WSL would otherwise parse as a URI with scheme "C".
_WINDOWS_DRIVE_PATH = re.compile(r"^[A-Za-z]:[\\/]")


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
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if r.returncode != 0:
            raise RuntimeError(f"WSL command failed: {r.stderr.strip()}")
        return r.stdout.strip()

    def _wsl_file_exists(self, path: str) -> bool:
        """True when `path` is a regular file inside the WSL distribution."""
        try:
            self._wsl(f"test -f {path}")
            return True
        except (RuntimeError, subprocess.SubprocessError, OSError):
            return False

    def _rebase_backing_file(self, wsl_disk: str) -> None:
        """Rewrite a session overlay's backing path into WSL's view of the filesystem.

        The overlay is created by the Windows-side builder, so its recorded backing
        file is a Windows path. QEMU inside WSL reads the drive letter as a URI scheme
        and refuses the drive with ``Could not open backing file: Unknown protocol 'C'``.
        The rebase is metadata-only (``-u``): it repoints the overlay without touching
        a byte of either image.
        """
        try:
            info = _json.loads(self._wsl(f"qemu-img info --output=json '{wsl_disk}'"))
        except (RuntimeError, ValueError) as exc:
            logger.debug("Could not inspect %s for a backing file: %s", wsl_disk, exc)
            return

        backing = info.get("backing-filename")
        if not backing or not _WINDOWS_DRIVE_PATH.match(backing):
            return

        wsl_backing = _win_to_wsl(backing)
        logger.info("Rebasing %s onto its WSL backing path %s", wsl_disk, wsl_backing)
        self._wsl(
            f"qemu-img rebase -u -f qcow2 -F qcow2 -b '{wsl_backing}' '{wsl_disk}'",
            timeout=120,
        )

    @staticmethod
    def available() -> bool:
        """Check if WSL2 + QEMU + KVM are available."""
        try:
            r = subprocess.run(
                ["wsl", "-e", "bash", "-c", "test -e /dev/kvm && which qemu-system-x86_64"],
                capture_output=True,
                timeout=10,
            )
            return r.returncode == 0
        except (subprocess.SubprocessError, FileNotFoundError):
            return False

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        ephemeral = opts.pop("ephemeral", True)

        if _native_eligible(image, opts, extra=bool(self.extra_args)):
            # The SDK's QEMU backend runs natively on Windows (WHPX).
            native = NativeQEMURuntime(
                ephemeral=ephemeral,
                cpus=self.cpu_count,
                memory_mb=self.memory_mb,
                server_port=getattr(self, "server_port", None),
            )
            self._native = native
            return await native.start(image, name, ephemeral=ephemeral, **opts)

        from cua_sandbox.builder.build import create_session_disk, has_build_work

        # Resolve disk path
        if not opts.get("disk_path") and not image._disk_path and image.kind == "vm":
            disk_path = str(await create_session_disk(image, name))
        elif has_build_work(image) and (image._disk_path or opts.get("disk_path")):
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
        self._rebase_backing_file(wsl_disk)
        disk_ext = Path(disk_path).suffix.lower()
        disk_fmt = {".qcow2": "qcow2", ".vhdx": "vhdx", ".raw": "raw", ".img": "raw"}.get(
            disk_ext, "raw"
        )

        # Locate OVMF inside WSL — code and vars must come from the same entry, or a
        # 4 MB firmware ends up backed by a 2 MB varstore and the guest cannot boot.
        ovmf_code, vars_template = (
            _locate_uefi_firmware_in(self._wsl_file_exists)
            if image.os_type == "windows"
            else (None, None)
        )

        # EFI vars — create in same dir as disk (WSL path)
        efivars_win = Path(disk_path).parent / "efivars.fd"
        wsl_efivars = _win_to_wsl(efivars_win)
        if ovmf_code and not efivars_win.exists():
            if vars_template is None:
                raise RuntimeError(
                    f"WSL has UEFI firmware at {ovmf_code} but no matching variable store. "
                    "Install the OVMF package inside the WSL distribution "
                    "(apt install ovmf) so the two halves match."
                )
            self._wsl(f"cp {vars_template} '{wsl_efivars}'")

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
            "-netdev "
            + _netdev(
                f"hostfwd=tcp:0.0.0.0:{hostfwd_port}-:{SPACESD_PORT}",
                restrict=opts.pop("network", None) == "none",
            ),
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
            capture_output=True,
            text=True,
            timeout=30,
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
        native = getattr(self, "_native", None)
        if native is not None:
            await native.delete(name)
            return
        try:
            subprocess.run(
                ["wsl", "-e", "bash", "-c", f"pkill -f 'qemu.*-name {name}'"],
                capture_output=True,
                timeout=10,
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
        # Daemon-agnostic: the VM process is up; the interfaces wait for
        # cua-spacesd themselves.
        return True


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
