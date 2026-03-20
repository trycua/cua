"""VNC + SSH transport — screenshots via VNC, shell commands via SSH.

VNC and SSH connect independently to the VM. No tunneling.
"""

from __future__ import annotations

import asyncio
import logging
import tempfile
from pathlib import Path
from typing import Any, Dict, Optional

import paramiko
from cua_sandbox.transport.base import Transport

logger = logging.getLogger(__name__)


class VNCSSHTransport(Transport):
    """Transport using SSH for commands and VNC for screenshots."""

    def __init__(
        self,
        *,
        ssh_host: str,
        ssh_port: int = 22,
        ssh_username: str = "admin",
        ssh_password: Optional[str] = "admin",
        ssh_key_filename: Optional[str] = None,
        vnc_host: str = "127.0.0.1",
        vnc_port: int = 5900,
        vnc_password: Optional[str] = None,
        environment: str = "linux",
    ):
        self._ssh_host = ssh_host
        self._ssh_port = ssh_port
        self._ssh_username = ssh_username
        self._ssh_password = ssh_password
        self._ssh_key_filename = ssh_key_filename
        self._vnc_host = vnc_host
        self._vnc_port = vnc_port
        self._vnc_password = vnc_password
        self._environment = environment
        self._ssh_client: Optional[paramiko.SSHClient] = None
        self._vnc_client: Any = None

    async def connect(self) -> None:
        loop = asyncio.get_event_loop()

        # Connect SSH
        self._ssh_client = paramiko.SSHClient()
        self._ssh_client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        await loop.run_in_executor(
            None,
            lambda: self._ssh_client.connect(
                self._ssh_host,
                port=self._ssh_port,
                username=self._ssh_username,
                password=self._ssh_password,
                key_filename=self._ssh_key_filename,
                timeout=30,
                look_for_keys=False,
                allow_agent=False,
            ),
        )
        logger.info(f"SSH connected to {self._ssh_host}:{self._ssh_port}")

        # Connect VNC directly
        from vncdotool import api as vnc_api

        self._vnc_client = await loop.run_in_executor(
            None,
            lambda: vnc_api.connect(
                f"{self._vnc_host}::{self._vnc_port}",
                password=self._vnc_password,
            ),
        )
        logger.info(f"VNC connected to {self._vnc_host}:{self._vnc_port}")

    async def get_display_url(self, *, share: bool = False) -> str:
        if share:
            raise NotImplementedError("share=True is not supported for local VNC transports.")
        return f"vnc://{self._vnc_host}:{self._vnc_port}"

    async def disconnect(self) -> None:
        if self._vnc_client:
            try:
                loop = asyncio.get_event_loop()
                await asyncio.wait_for(
                    loop.run_in_executor(None, self._vnc_client.disconnect),
                    timeout=5,
                )
            except Exception:
                pass
            self._vnc_client = None
            # Shut down the Twisted reactor thread used by vncdotool
            try:
                from vncdotool import api as vnc_api

                loop = asyncio.get_event_loop()
                await asyncio.wait_for(
                    loop.run_in_executor(None, vnc_api.shutdown),
                    timeout=5,
                )
            except Exception:
                pass
        if self._ssh_client:
            self._ssh_client.close()
            self._ssh_client = None

    async def send(self, action: str, **params: Any) -> Any:
        if not self._ssh_client:
            raise RuntimeError("SSH not connected")

        if action in ("shell", "run_command"):
            command = params.get("command", "")
            loop = asyncio.get_event_loop()
            _, stdout, stderr = await loop.run_in_executor(
                None,
                lambda: self._ssh_client.exec_command(command, timeout=params.get("timeout", 30)),
            )
            out = await loop.run_in_executor(None, stdout.read)
            err = await loop.run_in_executor(None, stderr.read)
            exit_code = stdout.channel.recv_exit_status()
            return {
                "stdout": out.decode(errors="replace"),
                "stderr": err.decode(errors="replace"),
                "returncode": exit_code,
            }
        raise NotImplementedError(f"VNCSSHTransport does not support action: {action}")

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        if not self._vnc_client:
            raise RuntimeError("VNC not connected")

        loop = asyncio.get_event_loop()
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
            tmp_path = tmp.name

        await loop.run_in_executor(None, self._vnc_client.captureScreen, tmp_path)
        data = Path(tmp_path).read_bytes()
        Path(tmp_path).unlink(missing_ok=True)
        from cua_sandbox.transport.base import convert_screenshot

        return convert_screenshot(data, format, quality)

    async def get_screen_size(self) -> Dict[str, int]:
        if not self._vnc_client:
            raise RuntimeError("VNC not connected")
        return {
            "width": self._vnc_client.screen.width,
            "height": self._vnc_client.screen.height,
        }

    async def get_environment(self) -> str:
        return self._environment
