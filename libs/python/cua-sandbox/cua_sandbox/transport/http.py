"""HTTPTransport — REST fallback for computer-server's POST /cmd endpoint.

The computer-server /cmd endpoint accepts JSON ``{"command": ..., "params": {...}}``
and returns an SSE stream with a single ``data: {...}`` frame containing the result.
"""

from __future__ import annotations

import base64
import json
from typing import Any, Dict, Optional

import httpx
from cua_sandbox.transport.base import Transport


class HTTPTransport(Transport):
    """Transport that communicates with computer-server over HTTP POST /cmd (SSE)."""

    def __init__(
        self,
        base_url: str,
        *,
        api_key: Optional[str] = None,
        container_name: Optional[str] = None,
        timeout: float = 30.0,
    ):
        """
        Args:
            base_url: Base URL of the computer-server, e.g. "http://localhost:8000".
            api_key: Optional API key (X-API-Key header) for cloud auth.
            container_name: Optional container name (X-Container-Name header) for cloud auth.
            timeout: HTTP request timeout in seconds.
        """
        self._base_url = base_url.rstrip("/")
        self._api_key = api_key
        self._container_name = container_name
        self._timeout = timeout
        self._client: Optional[httpx.AsyncClient] = None

    async def connect(self) -> None:
        headers: Dict[str, str] = {}
        if self._api_key:
            headers["X-API-Key"] = self._api_key
            headers["Authorization"] = f"Bearer {self._api_key}"
        if self._container_name:
            headers["X-Container-Name"] = self._container_name
        self._client = httpx.AsyncClient(
            base_url=self._base_url,
            headers=headers,
            timeout=self._timeout,
        )

    async def disconnect(self) -> None:
        if self._client:
            await self._client.aclose()
            self._client = None

    async def _cmd(self, command: str, params: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        """Send a command to POST /cmd and parse the SSE response."""
        assert self._client is not None, "Transport not connected"
        body = {"command": command}
        if params:
            body["params"] = params
        resp = await self._client.post("/cmd", json=body)
        resp.raise_for_status()
        return self._parse_sse(resp.text)

    @staticmethod
    def _parse_sse(text: str) -> Dict[str, Any]:
        """Extract the first ``data: {...}`` frame from an SSE response."""
        for line in text.splitlines():
            if line.startswith("data: "):
                payload = json.loads(line[6:])
                if isinstance(payload, dict) and not payload.get("success", True):
                    raise RuntimeError(f"Remote error: {payload.get('error', 'unknown')}")
                return payload
        raise RuntimeError(f"No SSE data frame in response: {text[:200]}")

    async def send(self, action: str, **params: Any) -> Any:
        result = await self._cmd(action, params if params else None)
        return result.get("result", result)

    async def screenshot(self) -> bytes:
        result = await self._cmd("screenshot")
        # computer-server returns {"success": true, "base64_image": "..."}
        b64 = result.get("image_data", result.get("base64_image", result.get("result", "")))
        if isinstance(b64, dict):
            b64 = b64.get("image_data", b64.get("base64_image", b64.get("base64", "")))
        return base64.b64decode(b64)

    async def get_screen_size(self) -> Dict[str, int]:
        result = await self._cmd("get_screen_size")
        # Flatten nested responses and normalize key names
        data = result
        if isinstance(data, dict):
            # Unwrap nested: {"result": {...}}, {"size": {...}}
            data = data.get("size", data.get("result", data))
        if isinstance(data, dict):
            w = data.get("width") or data.get("screen_width") or data.get("w")
            h = data.get("height") or data.get("screen_height") or data.get("h")
            if w is not None and h is not None:
                return {"width": int(w), "height": int(h)}
        raise KeyError(f"Cannot extract screen size from response: {result}")

    async def get_environment(self) -> str:
        # computer-server doesn't have a dedicated endpoint; use /status
        try:
            assert self._client is not None
            resp = await self._client.get("/status")
            resp.raise_for_status()
            data = resp.json()
            return data.get("os_type", data.get("platform", "linux"))
        except Exception:
            return "linux"
