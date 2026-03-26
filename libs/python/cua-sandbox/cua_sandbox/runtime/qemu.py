"""QEMU runtime — bare-metal or Docker-wrapped QEMU VMs.

Two modes:
  QEMURuntime(mode="docker")     — default, uses trycua/cua-qemu-* Docker images
  QEMURuntime(mode="bare-metal") — launches qemu-system-* directly on the host
"""

from __future__ import annotations

import asyncio
import json as _json
import logging
import platform as _plat
import shutil
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from cua_sandbox.image import Image

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

        # Platform — QEMU Windows/Android images are linux/amd64
        if image.os_type in ("windows", "android"):
            self.platform = "linux/amd64"

        # Android: forward ADB port (5555) in addition to API
        if image.os_type == "android":
            self.environment["ADB_PORT"] = "5555"

        # Longer boot timeout for Windows/Android VMs
        opts.pop("boot_timeout", None)

        info = await super().start(image, name, **opts)
        return info

    async def is_ready(self, info: RuntimeInfo, timeout: float = 300) -> bool:
        """Wait for the QEMU VM's computer-server to come up.

        Windows VMs take longer to boot (3-5 min), so default timeout is 300s.
        """
        return await super().is_ready(info, timeout=timeout)

    async def suspend(self, name: str) -> None:
        """Pause the Docker container running this QEMU VM."""
        subprocess.run(["docker", "pause", name], capture_output=True)

    async def resume(self, image: "Image", name: str, **opts) -> RuntimeInfo:
        """Unpause the Docker container and return RuntimeInfo."""
        subprocess.run(["docker", "unpause", name], capture_output=True)
        result = subprocess.run(
            [
                "docker",
                "inspect",
                "--format",
                '{{(index (index .NetworkSettings.Ports "8000/tcp") 0).HostPort}}',
                name,
            ],
            capture_output=True,
            text=True,
        )
        api_port = int(result.stdout.strip()) if result.stdout.strip().isdigit() else self.api_port
        info = RuntimeInfo(host="localhost", api_port=api_port, vnc_port=self.vnc_port, name=name)
        await self.is_ready(info)
        return info


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
    ):
        self.api_port = api_port
        self.vnc_display = vnc_display
        self.memory_mb = memory_mb
        self.cpu_count = cpu_count
        self.arch = arch
        self.qmp_port = qmp_port
        self.use_qmp_transport = use_qmp_transport
        self.extra_args = extra_args or []
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
        vnc_display = opts.get("vnc_display", self.vnc_display)
        hostfwd_port = opts.get("api_port") or _find_free_port(self.api_port)
        enable_kvm = opts.get("enable_kvm", True)

        # Detect guest server port from transport hint
        guest_port = 5000 if image._agent_type in ("osworld", "androidworld") else 8000

        # Detect disk format from extension
        disk_ext = Path(disk_path).suffix.lower()
        disk_fmt = {".qcow2": "qcow2", ".vhdx": "vhdx", ".raw": "raw", ".img": "raw"}.get(
            disk_ext, "raw"
        )

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
                f"user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:{hostfwd_port}-:{guest_port}",
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
            cmd += ["-qmp", f"tcp:127.0.0.1:{self.qmp_port},server,nowait"]

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
            vnc_port=5900 + vnc_display,
            name=name,
            qmp_port=self.qmp_port if use_qmp else None,
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
                vnc_port=5900 + vnc_display,
                qmp_port=self.qmp_port,
                disk_path=str(disk_path) if disk_path else None,
                os_type=image.os_type,
                vnc_display=vnc_display,
                memory_mb=memory,
                cpu_count=cpus,
                arch=self.arch,
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
            (
                f"user,id=net0,"
                f"hostfwd=tcp:127.0.0.1:{hostfwd_port}-:8000,"
                f"hostfwd=tcp:127.0.0.1:{adb_port}-:5555"
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
        cmd += ["-qmp", f"tcp:127.0.0.1:{self.qmp_port},server,nowait"]

        # Daemonize on Unix
        if _plat.system() != "Windows":
            cmd.append("-daemonize")

        return cmd

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
        if info.qmp_port and not info.agent_type:
            return await self._is_ready_qmp(info, timeout)
        # For OSWorld/AndroidWorld, check the server health; for computer-server, check /status
        if info.agent_type == "osworld":
            endpoint = "/screenshot"
        elif info.agent_type == "androidworld":
            endpoint = "/health"
        else:
            endpoint = "/status"
        url = f"http://{info.host}:{info.api_port}{endpoint}"
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
        guest_port = 8000

        cmd += [
            "-drive",
            f"file={disk_path},format={disk_fmt},if=virtio",
            "-netdev",
            f"user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:{api_port}-:{guest_port}",
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
        disk_fmt = {".qcow2": "qcow2", ".vhdx": "vhdx", ".raw": "raw", ".img": "raw"}.get(
            disk_ext, "raw"
        )

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
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"WSL2 QEMU VM {info.name} is ready")
                        return True
                except (
                    httpx.ConnectError,
                    httpx.ReadTimeout,
                    httpx.RemoteProtocolError,
                    httpx.ConnectTimeout,
                ):
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
