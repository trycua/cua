"""GRPCEmulatorTransport — screenshots via the Android emulator's built-in gRPC service.

The emulator exposes an EmulatorController gRPC service on console_port+3000
(default 8554 for an emulator on console port 5554). This bypasses ADB entirely,
reducing screenshot latency from ~500ms to ~50ms.

Launch the emulator with -grpc <port> or rely on the default console_port+3000.
"""

from __future__ import annotations

from io import BytesIO
from typing import Any, Dict, Optional

import grpc
from cua_sandbox.transport._grpc_emulator import emulator_controller_pb2 as pb2
from cua_sandbox.transport._grpc_emulator import (
    emulator_controller_pb2_grpc as pb2_grpc,
)
from cua_sandbox.transport.base import Transport


class GRPCEmulatorTransport(Transport):
    """Transport backed by the Android emulator's gRPC EmulatorController service."""

    def __init__(self, host: str = "localhost", grpc_port: int = 8554):
        self._host = host
        self._grpc_port = grpc_port
        self._channel: Optional[grpc.aio.Channel] = None
        self._stub: Optional[pb2_grpc.EmulatorControllerStub] = None

    async def connect(self) -> None:
        self._channel = grpc.aio.insecure_channel(f"{self._host}:{self._grpc_port}")
        self._stub = pb2_grpc.EmulatorControllerStub(self._channel)

    async def disconnect(self) -> None:
        if self._channel:
            await self._channel.close()
            self._channel = None
            self._stub = None

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        assert self._stub is not None, "Transport not connected"
        fmt = format.lower()

        if fmt in ("jpeg", "jpg"):
            # Request raw RGB888 — no PNG overhead, minimal data, fast PIL JPEG encode
            img_fmt = pb2.ImageFormat(format=pb2.ImageFormat.RGB888)
            response = await self._stub.getScreenshot(img_fmt)
            w, h = response.format.width, response.format.height
            rgb = bytes(response.image)
            from PIL import Image as PILImage

            img = PILImage.frombytes("RGB", (w, h), rgb)
            buf = BytesIO()
            img.save(buf, format="JPEG", quality=quality, optimize=True)
            return buf.getvalue()

        # PNG: request PNG directly from the emulator
        img_fmt = pb2.ImageFormat(format=pb2.ImageFormat.PNG)
        response = await self._stub.getScreenshot(img_fmt)
        return bytes(response.image)

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._stub is not None, "Transport not connected"
        img_fmt = pb2.ImageFormat(format=pb2.ImageFormat.PNG, width=1, height=1)
        response = await self._stub.getScreenshot(img_fmt)
        return {"width": response.format.width, "height": response.format.height}

    async def get_environment(self) -> str:
        return "android"

    async def send(self, action: str, **params: Any) -> Any:
        raise NotImplementedError(
            f"GRPCEmulatorTransport.send({action!r}) not implemented — use ADBTransport for shell/input"
        )
