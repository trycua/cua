"""SSH transport — shell commands and file transfer over SSH.

Screenshots are not natively supported; pair with VNCSSHTransport for
full desktop interaction over SSH-tunneled VNC.
"""

from __future__ import annotations

import logging
import socket
import threading
from typing import TYPE_CHECKING, Any, Dict, List, Optional

if TYPE_CHECKING:
    from cua_sandbox.interfaces.tunnel import TunnelInfo

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
        self._tunnels: List[_SSHTunnel] = []

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
        for t in list(self._tunnels):
            t.stop()
        self._tunnels.clear()
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

    # ── Tunnel ────────────────────────────────────────────────────────────────

    async def forward_tunnel(self, sandbox_port: int) -> "TunnelInfo":
        """Open an SSH local-forward from a free host port to *sandbox_port* on the remote."""
        import asyncio

        from cua_sandbox.interfaces.tunnel import TunnelInfo

        if not self._client:
            raise RuntimeError("SSH not connected")

        loop = asyncio.get_event_loop()
        tunnel = await loop.run_in_executor(
            None, lambda: _SSHTunnel.start(self._client, "localhost", sandbox_port)
        )
        self._tunnels.append(tunnel)
        info = TunnelInfo(host="localhost", port=tunnel.local_port, sandbox_port=sandbox_port)
        return info

    async def close_tunnel(self, info: "TunnelInfo") -> None:
        import asyncio

        to_close = [t for t in self._tunnels if t.local_port == info.port]
        loop = asyncio.get_event_loop()
        for t in to_close:
            await loop.run_in_executor(None, t.stop)
            self._tunnels.remove(t)


class _SSHTunnel:
    """Minimal SSH local-forward: binds a random localhost port, pipes to remote."""

    def __init__(self, local_port: int, server_sock: socket.socket):
        self.local_port = local_port
        self._server_sock = server_sock
        self._stop_event = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)

    @classmethod
    def start(
        cls,
        ssh_client: paramiko.SSHClient,
        remote_host: str,
        remote_port: int,
    ) -> "_SSHTunnel":
        server_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server_sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server_sock.bind(("127.0.0.1", 0))
        local_port = server_sock.getsockname()[1]
        server_sock.listen(5)
        server_sock.settimeout(1.0)

        tunnel = cls(local_port, server_sock)
        tunnel._ssh_client = ssh_client
        tunnel._remote_host = remote_host
        tunnel._remote_port = remote_port
        tunnel._thread.start()
        return tunnel

    def stop(self) -> None:
        self._stop_event.set()
        self._server_sock.close()

    def _serve(self) -> None:
        while not self._stop_event.is_set():
            try:
                client_sock, _ = self._server_sock.accept()
            except OSError:
                break
            transport = self._ssh_client.get_transport()
            if transport is None:
                client_sock.close()
                break
            try:
                channel = transport.open_channel(
                    "direct-tcpip",
                    (self._remote_host, self._remote_port),
                    client_sock.getpeername(),
                )
            except Exception:
                client_sock.close()
                continue
            threading.Thread(target=self._pipe, args=(client_sock, channel), daemon=True).start()

    @staticmethod
    def _pipe(sock: socket.socket, channel: paramiko.Channel) -> None:
        import select

        channel.setblocking(False)
        sock.setblocking(False)
        try:
            while True:
                r, _, _ = select.select([sock, channel], [], [], 1.0)
                if sock in r:
                    data = sock.recv(4096)
                    if not data:
                        break
                    channel.sendall(data)
                if channel in r:
                    data = channel.recv(4096)
                    if not data:
                        break
                    sock.sendall(data)
        finally:
            sock.close()
            channel.close()
