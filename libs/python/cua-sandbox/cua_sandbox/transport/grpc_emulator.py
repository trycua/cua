"""GRPCEmulatorTransport — screenshots and input via the Android emulator's gRPC service.

The emulator exposes an EmulatorController gRPC service on console_port+3000
(default 8554 for an emulator on console port 5554). This bypasses ADB entirely,
reducing screenshot latency from ~500ms to ~20ms.

Launch the emulator with -grpc <port> or rely on the default console_port+3000.

Uses the synchronous grpc channel (not grpc.aio) so the stub is safe to call
from any event loop via run_in_executor — avoids the "Future attached to a
different loop" error that grpc.aio channels produce in pytest session fixtures.
"""

from __future__ import annotations

import asyncio
import functools
import shutil
import subprocess
from io import BytesIO
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, List, Optional

import grpc

# google.protobuf must be imported before the emulator pb2 stubs — do not reorder.
from google.protobuf import empty_pb2  # noqa: F401 isort: skip

# isort: split
from cua_sandbox.transport._grpc_emulator import emulator_controller_pb2 as pb2
from cua_sandbox.transport._grpc_emulator import (
    emulator_controller_pb2_grpc as pb2_grpc,
)
from cua_sandbox.transport.base import Transport

if TYPE_CHECKING:
    from cua_sandbox.interfaces.tunnel import TunnelInfo


def _find_adb(sdk_root: Optional[str] = None) -> str:
    if sdk_root:
        candidate = Path(sdk_root) / "platform-tools" / "adb"
        if candidate.exists():
            return str(candidate)
    found = shutil.which("adb")
    if found:
        return found
    raise FileNotFoundError("adb not found. Install the Android SDK or set ANDROID_HOME.")


class GRPCEmulatorTransport(Transport):
    """Transport backed by the Android emulator's gRPC EmulatorController service.

    Uses a synchronous gRPC channel so it is safe across multiple event loops
    (e.g. pytest session fixtures). All blocking RPC calls are offloaded to a
    thread executor.
    """

    def __init__(
        self,
        host: str = "localhost",
        grpc_port: int = 8554,
        serial: str = "emulator-5554",
        sdk_root: Optional[str] = None,
    ):
        self._host = host
        self._grpc_port = grpc_port
        self._serial = serial
        self._sdk_root = sdk_root
        self._adb: Optional[str] = None
        self._channel: Optional[grpc.Channel] = None
        self._stub: Optional[pb2_grpc.EmulatorControllerStub] = None

    # ── Lifecycle ─────────────────────────────────────────────────────────────

    async def connect(self) -> None:
        self._adb = _find_adb(self._sdk_root)
        self._channel = grpc.insecure_channel(
            f"{self._host}:{self._grpc_port}",
            options=[
                ("grpc.max_receive_message_length", 32 * 1024 * 1024),
                ("grpc.max_send_message_length", 32 * 1024 * 1024),
            ],
        )
        self._stub = pb2_grpc.EmulatorControllerStub(self._channel)

    async def disconnect(self) -> None:
        if self._channel:
            self._channel.close()
            self._channel = None
            self._stub = None

    # ── Helpers ───────────────────────────────────────────────────────────────

    async def _rpc(self, fn, *args, **kwargs):
        """Run a synchronous gRPC call in the thread executor."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(None, functools.partial(fn, *args, **kwargs))

    async def _send_touch(self, event: pb2.TouchEvent) -> None:
        await self._rpc(self._stub.sendTouch, event)

    # ── Transport interface ───────────────────────────────────────────────────

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        assert self._stub is not None, "Transport not connected"
        fmt = format.lower()

        if fmt in ("jpeg", "jpg"):
            img_fmt = pb2.ImageFormat(format=pb2.ImageFormat.RGB888)
            response = await self._rpc(self._stub.getScreenshot, img_fmt)
            w, h = response.format.width, response.format.height
            from PIL import Image as PILImage

            img = PILImage.frombytes("RGB", (w, h), bytes(response.image))
            buf = BytesIO()
            img.save(buf, format="JPEG", quality=quality, optimize=True)
            return buf.getvalue()

        img_fmt = pb2.ImageFormat(format=pb2.ImageFormat.PNG)
        response = await self._rpc(self._stub.getScreenshot, img_fmt)
        return bytes(response.image)

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._stub is not None, "Transport not connected"
        img_fmt = pb2.ImageFormat(format=pb2.ImageFormat.PNG)
        response = await self._rpc(self._stub.getScreenshot, img_fmt)
        return {"width": response.format.width, "height": response.format.height}

    async def get_environment(self) -> str:
        return "android"

    async def send(self, action: str, **params: Any) -> Any:
        assert self._stub is not None, "Transport not connected"

        if action in ("left_click", "right_click", "double_click"):
            x, y = int(params["x"]), int(params["y"])
            n_taps = 2 if action == "double_click" else 1
            for _ in range(n_taps):
                await self._send_touch(
                    pb2.TouchEvent(touches=[pb2.Touch(x=x, y=y, identifier=0, pressure=1)])
                )
                await self._send_touch(
                    pb2.TouchEvent(touches=[pb2.Touch(x=x, y=y, identifier=0, pressure=0)])
                )
            return {}

        if action == "mouse_down":
            x, y = int(params["x"]), int(params["y"])
            await self._send_touch(
                pb2.TouchEvent(touches=[pb2.Touch(x=x, y=y, identifier=0, pressure=1)])
            )
            return {}

        if action == "mouse_up":
            x, y = int(params["x"]), int(params["y"])
            await self._send_touch(
                pb2.TouchEvent(touches=[pb2.Touch(x=x, y=y, identifier=0, pressure=0)])
            )
            return {}

        if action == "move_cursor":
            return {}

        if action in ("shell", "execute", "run_command"):
            assert self._adb is not None, "Transport not connected"
            cmd = params.get("command", "")
            timeout = float(params.get("timeout", 15))
            loop = asyncio.get_event_loop()
            result = await loop.run_in_executor(
                None,
                functools.partial(
                    subprocess.run,
                    [self._adb, "-s", self._serial, "shell", cmd],
                    capture_output=True,
                    timeout=timeout,
                    check=False,
                ),
            )
            return {
                "stdout": result.stdout.decode(errors="replace"),
                "stderr": result.stderr.decode(errors="replace"),
                "returncode": result.returncode,
            }

        if action == "multitouch_gesture":
            # Send all fingers simultaneously per frame via sendTouch.
            fingers: List[Dict] = params["fingers"]
            steps: int = max(1, int(params.get("steps", 10)))
            duration_ms: float = float(params.get("duration_ms", 400))
            delay = (duration_ms / 1000.0) / steps

            # Press all fingers simultaneously
            await self._send_touch(
                pb2.TouchEvent(
                    touches=[
                        pb2.Touch(
                            x=int(f["start"][0]),
                            y=int(f["start"][1]),
                            identifier=i,
                            pressure=1,
                        )
                        for i, f in enumerate(fingers)
                    ]
                )
            )

            # Interpolate movement frames
            for step in range(1, steps + 1):
                t = step / steps
                await self._send_touch(
                    pb2.TouchEvent(
                        touches=[
                            pb2.Touch(
                                x=int(f["start"][0] + t * (f["end"][0] - f["start"][0])),
                                y=int(f["start"][1] + t * (f["end"][1] - f["start"][1])),
                                identifier=i,
                                pressure=1,
                            )
                            for i, f in enumerate(fingers)
                        ]
                    )
                )
                await asyncio.sleep(delay)

            # Release all fingers simultaneously
            await self._send_touch(
                pb2.TouchEvent(
                    touches=[
                        pb2.Touch(
                            x=int(f["end"][0]),
                            y=int(f["end"][1]),
                            identifier=i,
                            pressure=0,
                        )
                        for i, f in enumerate(fingers)
                    ]
                )
            )
            return {}

        raise NotImplementedError(f"GRPCEmulatorTransport.send({action!r}) not implemented")

    # ── Tunnel ────────────────────────────────────────────────────────────────

    async def forward_tunnel(self, sandbox_port: int | str) -> "TunnelInfo":
        """Forward a sandbox TCP port or abstract socket to a free host port.

        - ``int`` → ``adb forward tcp:0 tcp:<sandbox_port>``
        - ``str`` → ``adb forward tcp:0 localabstract:<sandbox_port>``
          (e.g. ``"chrome_devtools_remote"`` for Chrome DevTools)
        """
        from cua_sandbox.interfaces.tunnel import TunnelInfo

        target = (
            f"localabstract:{sandbox_port}"
            if isinstance(sandbox_port, str)
            else f"tcp:{sandbox_port}"
        )
        loop = asyncio.get_event_loop()
        result = await loop.run_in_executor(
            None,
            functools.partial(
                subprocess.run,
                [self._adb, "-s", self._serial, "forward", "tcp:0", target],
                capture_output=True,
                check=False,
            ),
        )
        if result.returncode != 0:
            raise RuntimeError(f"adb forward failed: {result.stderr.decode(errors='replace')}")
        host_port = int(result.stdout.decode().strip())
        return TunnelInfo(host="localhost", port=host_port, sandbox_port=sandbox_port)

    async def close_tunnel(self, info: "TunnelInfo") -> None:
        loop = asyncio.get_event_loop()
        await loop.run_in_executor(
            None,
            functools.partial(
                subprocess.run,
                [self._adb, "-s", self._serial, "forward", "--remove", f"tcp:{info.port}"],
                capture_output=True,
                check=False,
            ),
        )
