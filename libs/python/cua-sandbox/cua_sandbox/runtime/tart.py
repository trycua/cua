"""Tart runtime — uses Cirrus Labs' Tart CLI for Apple VZ framework VMs.

Tart manages macOS and Linux VMs using Apple's Virtualization.framework.
It supports pulling OCI images from registries (ghcr.io, etc.), running
headless VMs with VNC on the host side, and SSH into the guest.

Default SSH credentials for Tart VMs: admin/admin.
VNC is exposed on a random localhost port with a random password (parsed from stderr).

Usage::

    from cua_sandbox.runtime import TartRuntime

    async with sandbox(
        local=True,
        image=Image.from_registry("ghcr.io/cirruslabs/macos-tahoe-base:latest"),
        runtime=TartRuntime(),
        name="my-macos-vm",
    ) as sb:
        await sb.shell.run("uname -a")
        screenshot = await sb.screenshot()
"""

from __future__ import annotations

import asyncio
import logging
import re
import shutil
from typing import Optional

from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo

logger = logging.getLogger(__name__)

TART_DEFAULT_USERNAME = "admin"
TART_DEFAULT_PASSWORD = "admin"


def _has_tart() -> bool:
    return shutil.which("tart") is not None


class TartRuntime(Runtime):
    """Apple Virtualization.framework runtime via Tart CLI.

    Uses VNC (host-side, random port) for screenshots and SSH for shell commands.
    """

    def __init__(
        self,
        *,
        ephemeral: bool = True,
        cpu_count: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        display: str = "1024x768",
        ssh_username: str = TART_DEFAULT_USERNAME,
        ssh_password: str = TART_DEFAULT_PASSWORD,
    ):
        self.ephemeral = ephemeral
        self.cpu_count = cpu_count
        self.memory_mb = memory_mb
        self.disk_gb = disk_gb
        self.display = display
        self.ssh_username = ssh_username
        self.ssh_password = ssh_password
        self._proc: Optional[asyncio.subprocess.Process] = None
        self._vm_name: Optional[str] = None
        self.vnc_host: Optional[str] = None
        self.vnc_port: Optional[int] = None
        self.vnc_password: Optional[str] = None

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if not _has_tart():
            raise RuntimeError("Tart CLI not found. Install via: brew install cirruslabs/cli/tart")

        registry_ref = image._registry
        if not registry_ref:
            raise ValueError(
                "TartRuntime requires an OCI registry image. "
                "Use Image.from_registry('ghcr.io/...') to specify one."
            )

        # Pull the image
        logger.info(f"Pulling {registry_ref} ...")
        pull = await asyncio.create_subprocess_exec(
            "tart",
            "pull",
            registry_ref,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        _, stderr = await pull.communicate()
        if pull.returncode != 0:
            raise RuntimeError(f"tart pull failed: {stderr.decode()}")

        # Clone for ephemeral use
        self._vm_name = name
        if self.ephemeral:
            del_proc = await asyncio.create_subprocess_exec(
                "tart",
                "delete",
                name,
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.DEVNULL,
            )
            await del_proc.communicate()

            clone = await asyncio.create_subprocess_exec(
                "tart",
                "clone",
                registry_ref,
                name,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            _, stderr = await clone.communicate()
            if clone.returncode != 0:
                raise RuntimeError(f"tart clone failed: {stderr.decode()}")

        # Configure VM resources
        set_args = []
        if self.cpu_count:
            set_args += ["--cpu", str(self.cpu_count)]
        if self.memory_mb:
            set_args += ["--memory", str(self.memory_mb)]
        if self.disk_gb:
            set_args += ["--disk-size", str(self.disk_gb)]
        if self.display:
            set_args += ["--display", self.display]
        if set_args:
            set_proc = await asyncio.create_subprocess_exec(
                "tart",
                "set",
                name,
                *set_args,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            await set_proc.communicate()

        # Start VM headless with VNC
        run_cmd = ["tart", "run", name, "--no-graphics", "--vnc-experimental"]
        logger.info(f"Starting VM: {' '.join(run_cmd)}")
        self._proc = await asyncio.create_subprocess_exec(
            *run_cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        # Parse VNC URL from stdout (e.g. "vnc://:password@127.0.0.1:61777")
        vnc_line = await self._read_vnc_line(timeout=10)
        if vnc_line:
            m = re.search(r"vnc://:([^@]+)@([^:]+):(\d+)", vnc_line)
            if m:
                self.vnc_password = m.group(1)
                self.vnc_host = m.group(2)
                self.vnc_port = int(m.group(3))
                logger.info(f"VNC available at {self.vnc_host}:{self.vnc_port}")

        if not self.vnc_port:
            raise RuntimeError("Could not parse VNC port from tart output")

        # Brief check that it didn't crash
        if self._proc.returncode is not None:
            err = await self._proc.stderr.read() if self._proc.stderr else b""
            raise RuntimeError(f"Tart VM crashed on launch: {err.decode()}")

        # Get VM IP for SSH
        ip = await self._get_ip(name, timeout=120)
        if not ip:
            raise RuntimeError(f"Could not get IP for Tart VM {name}")

        env = "mac" if "macos" in registry_ref.lower() or "mac" in registry_ref.lower() else "linux"

        return RuntimeInfo(
            host=ip,
            api_port=22,
            vnc_port=self.vnc_port,
            vnc_host=self.vnc_host or "127.0.0.1",
            vnc_password=self.vnc_password,
            ssh_port=22,
            ssh_username=self.ssh_username,
            ssh_password=self.ssh_password,
            name=name,
            environment=env,
        )

    async def _read_vnc_line(self, timeout: float = 10) -> Optional[str]:
        """Read stdout lines until we find the VNC URL."""
        if not self._proc or not self._proc.stdout:
            return None
        try:
            deadline = asyncio.get_event_loop().time() + timeout
            while asyncio.get_event_loop().time() < deadline:
                line = await asyncio.wait_for(
                    self._proc.stdout.readline(),
                    timeout=max(0.1, deadline - asyncio.get_event_loop().time()),
                )
                text = line.decode().strip()
                if "vnc://" in text.lower():
                    return text
        except (asyncio.TimeoutError, Exception):
            pass
        return None

    async def _get_ip(self, name: str, timeout: int = 120) -> Optional[str]:
        try:
            proc = await asyncio.create_subprocess_exec(
                "tart",
                "ip",
                name,
                "--wait",
                str(timeout),
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout + 10)
            ip = stdout.decode().strip()
            if ip:
                logger.info(f"VM {name} IP: {ip}")
                return ip
        except (asyncio.TimeoutError, Exception) as e:
            logger.warning(f"Could not get VM IP: {e}")
        return None

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        """Wait until SSH is reachable."""
        import socket

        deadline = asyncio.get_event_loop().time() + timeout
        while asyncio.get_event_loop().time() < deadline:
            try:
                sock = socket.create_connection((info.host, 22), timeout=5)
                sock.close()
                logger.info(f"Tart VM {info.name} SSH is reachable")
                return True
            except (ConnectionRefusedError, OSError, socket.timeout):
                pass
            await asyncio.sleep(3)
        raise TimeoutError(f"Tart VM {info.name} SSH not ready within {timeout}s")

    async def stop(self, name: str) -> None:
        stop = await asyncio.create_subprocess_exec(
            "tart",
            "stop",
            name,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        await stop.communicate()

        if self._proc and self._proc.returncode is None:
            self._proc.terminate()
            try:
                await asyncio.wait_for(self._proc.communicate(), timeout=10)
            except asyncio.TimeoutError:
                self._proc.kill()
            self._proc = None

        if self.ephemeral:
            await asyncio.create_subprocess_exec(
                "tart",
                "delete",
                name,
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.DEVNULL,
            )
            logger.info(f"Deleted ephemeral VM: {name}")
