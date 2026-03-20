"""WebSocketTransport — connects to a computer-server instance via WebSocket.

The computer-server exposes a WebSocket endpoint that accepts JSON commands
and returns JSON responses. Screenshots are returned as base64-encoded PNG.
"""

from __future__ import annotations

import base64
import json
from typing import Any, Dict, Optional

import websockets
from cua_sandbox.transport.base import Transport
from websockets.asyncio.client import ClientConnection


class WebSocketTransport(Transport):
    """Transport that communicates with computer-server over WebSocket."""

    def __init__(self, url: str, api_key: Optional[str] = None):
        """
        Args:
            url: WebSocket URL, e.g. "ws://localhost:8000/ws"
            api_key: Optional API key for authentication.
        """
        self._url = url
        self._api_key = api_key
        self._ws: Optional[ClientConnection] = None

    async def connect(self) -> None:
        headers = {}
        if self._api_key:
            headers["Authorization"] = f"Bearer {self._api_key}"
        self._ws = await websockets.connect(self._url, additional_headers=headers)

    async def disconnect(self) -> None:
        if self._ws:
            await self._ws.close()
            self._ws = None

    async def _request(self, payload: dict) -> Any:
        assert self._ws is not None, "Transport not connected"
        await self._ws.send(json.dumps(payload))
        raw = await self._ws.recv()
        return json.loads(raw)

    async def send(self, action: str, **params: Any) -> Any:
        resp = await self._request({"command": action, **params})
        if isinstance(resp, dict) and resp.get("error"):
            raise RuntimeError(f"Remote error: {resp['error']}")
        return resp.get("result") if isinstance(resp, dict) else resp

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        resp = await self._request({"command": "screenshot"})
        b64 = resp.get("result", resp.get("screenshot", ""))
        if isinstance(b64, dict):
            b64 = b64.get("base64", "")
        png = base64.b64decode(b64)
        from cua_sandbox.transport.base import convert_screenshot

        return convert_screenshot(png, format, quality)

    async def get_screen_size(self) -> Dict[str, int]:
        resp = await self._request({"command": "get_screen_size"})
        result = resp.get("result", resp)
        return {"width": result["width"], "height": result["height"]}

    async def get_environment(self) -> str:
        resp = await self._request({"command": "get_environment"})
        return resp.get("result", "linux")
