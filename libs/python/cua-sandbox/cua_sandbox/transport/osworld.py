"""OSWorldTransport — speaks the OSWorld Flask server API (port 5000).

The OSWorld computer-server exposes:
  GET  /screenshot        → raw PNG bytes
  POST /screen_size       → {"width": ..., "height": ...}
  POST /execute           → {"command": [...], "shell": false} → {"output": ...}
  POST /run_bash_script   → {"script": ..., "timeout": ...} → {"status": ..., "output": ..., "error": ..., "returncode": ...}
  GET  /accessibility      → {"AT": ...}
"""

from __future__ import annotations

from typing import Any, Dict, Optional

import httpx
from cua_sandbox.transport.base import Transport


class OSWorldTransport(Transport):
    """Transport for VMs running the OSWorld Flask server (pyautogui-based)."""

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 30.0,
    ):
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._client: Optional[httpx.AsyncClient] = None

    async def connect(self) -> None:
        self._client = httpx.AsyncClient(
            base_url=self._base_url,
            timeout=self._timeout,
        )

    async def disconnect(self) -> None:
        if self._client:
            await self._client.aclose()
            self._client = None

    async def send(self, action: str, **params: Any) -> Any:
        assert self._client is not None, "Transport not connected"
        if action in ("execute", "run_bash_script", "run_python"):
            resp = await self._client.post(f"/{action}", json=params)
            resp.raise_for_status()
            return resp.json()
        if action == "accessibility":
            resp = await self._client.get("/accessibility")
            resp.raise_for_status()
            return resp.json()
        if action == "terminal":
            resp = await self._client.get("/terminal")
            resp.raise_for_status()
            return resp.json()
        raise ValueError(f"Unknown action: {action}")

    async def screenshot(self) -> bytes:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get("/screenshot")
        resp.raise_for_status()
        return resp.content

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post("/screen_size")
        resp.raise_for_status()
        data = resp.json()
        return {"width": int(data["width"]), "height": int(data["height"])}

    async def get_environment(self) -> str:
        return "linux"
