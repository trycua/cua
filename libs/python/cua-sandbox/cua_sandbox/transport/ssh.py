"""SSH transport — shell commands and file transfer over SSH.

Screenshots are not natively supported; pair with VNCSSHTransport for
full desktop interaction over SSH-tunneled VNC.
"""

from __future__ import annotations

import logging
from typing import Any, Dict, Optional

import paramiko
from cua_sandbox.transport.base import Transport

logger = logging.getLogger(__name__)


class SSHTransport(Transport):
    """Transport that executes commands over SSH using paramiko."""

    def __init__(
        self,
        host: str,
        port: int = 22,
        username: str = "admin",
        password: Optional[str] = "admin",
        key_filename: Optional[str] = None,
        environment: str = "linux",
    ):
        self._host = host
        self._port = port
        self._username = username
        self._password = password
        self._key_filename = key_filename
        self._environment = environment
        self._client: Optional[paramiko.SSHClient] = None

    async def connect(self) -> None:
        import asyncio

        loop = asyncio.get_event_loop()
        self._client = paramiko.SSHClient()
        self._client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        await loop.run_in_executor(
            None,
            lambda: self._client.connect(
                self._host,
                port=self._port,
                username=self._username,
                password=self._password,
                key_filename=self._key_filename,
                timeout=30,
                look_for_keys=False,
                allow_agent=False,
            ),
        )
        logger.info(f"SSH connected to {self._host}:{self._port}")

    async def disconnect(self) -> None:
        if self._client:
            self._client.close()
            self._client = None

    async def send(self, action: str, **params: Any) -> Any:
        import asyncio

        if not self._client:
            raise RuntimeError("SSH not connected")

        if action in ("shell", "run_command"):
            command = params.get("command", "")
            loop = asyncio.get_event_loop()
            _, stdout, stderr = await loop.run_in_executor(
                None, lambda: self._client.exec_command(command, timeout=params.get("timeout", 30))
            )
            out = await loop.run_in_executor(None, stdout.read)
            err = await loop.run_in_executor(None, stderr.read)
            exit_code = stdout.channel.recv_exit_status()
            return {
                "stdout": out.decode(errors="replace"),
                "stderr": err.decode(errors="replace"),
                "returncode": exit_code,
            }
        raise NotImplementedError(f"SSH transport does not support action: {action}")

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        raise NotImplementedError(
            "SSH transport does not support screenshots. "
            "Use VNCSSHTransport for screenshot support over SSH-tunneled VNC."
        )

    async def get_screen_size(self) -> Dict[str, int]:
        raise NotImplementedError("SSH transport does not support screen size queries.")

    async def get_environment(self) -> str:
        return self._environment
