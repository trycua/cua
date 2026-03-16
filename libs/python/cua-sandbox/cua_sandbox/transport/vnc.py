"""VNC transport — uses vncdotool for screenshots and input over VNC.

Used by the Tart runtime for headless VMs that expose VNC but don't
run computer-server.
"""

from __future__ import annotations

import asyncio
import logging
import tempfile
from pathlib import Path
from typing import Any, Dict, Optional

from cua_sandbox.transport.base import Transport

logger = logging.getLogger(__name__)


class VNCTransport(Transport):
    """Transport that connects to a VM over VNC using vncdotool."""

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 5900,
        password: Optional[str] = None,
        environment: str = "linux",
    ):
        self._host = host
        self._port = port
        self._password = password
        self._environment = environment
        self._client: Any = None

    async def connect(self) -> None:
        from vncdotool import api as vnc_api

        loop = asyncio.get_event_loop()
        self._client = await loop.run_in_executor(
            None,
            lambda: vnc_api.connect(
                f"{self._host}::{self._port}",
                password=self._password,
            ),
        )
        logger.info(f"VNC connected to {self._host}:{self._port}")

    async def disconnect(self) -> None:
        if self._client:
            try:
                loop = asyncio.get_event_loop()
                await asyncio.wait_for(
                    loop.run_in_executor(None, self._client.disconnect),
                    timeout=5,
                )
            except Exception:
                pass
            self._client = None
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

    async def send(self, action: str, **params: Any) -> Any:
        if action == "shell":
            raise NotImplementedError("Shell commands not supported over VNC")
        raise NotImplementedError(f"VNC transport does not support action: {action}")

    async def screenshot(self) -> bytes:
        if not self._client:
            raise RuntimeError("VNC not connected")

        loop = asyncio.get_event_loop()
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
            tmp_path = tmp.name

        await loop.run_in_executor(None, self._client.captureScreen, tmp_path)
        data = Path(tmp_path).read_bytes()
        Path(tmp_path).unlink(missing_ok=True)
        return data

    async def get_screen_size(self) -> Dict[str, int]:
        if not self._client:
            raise RuntimeError("VNC not connected")
        # vncdotool exposes screen dimensions via the client
        return {
            "width": self._client.screen.width,
            "height": self._client.screen.height,
        }

    async def get_environment(self) -> str:
        return self._environment
