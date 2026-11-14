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

OSWORLD_SERVER_PORT = 5000
"""Port the OSWorld Flask control server listens on inside the guest."""


def normalize_execute_result(payload: Dict[str, Any]) -> Dict[str, Any]:
    """Shape an OSWorld ``/execute`` (or ``/run_bash_script``) reply like a computer-server result."""
    returncode = payload.get("returncode")
    if returncode is None:
        returncode = 0 if payload.get("status") == "success" else 1
    return {
        "stdout": payload.get("output", "") or "",
        "stderr": payload.get("error", payload.get("message", "")) or "",
        "returncode": returncode,
    }


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
        if action == "run_command":
            # Sandbox.shell.run() speaks the computer-server vocabulary; map it
            # onto the OSWorld server's /execute (shell=true), which every
            # OSWorld server generation exposes (the golden image predates
            # /run_bash_script).
            timeout = float(params.get("timeout", 30))
            resp = await self._client.post(
                "/execute",
                json={"command": params["command"], "shell": True},
                timeout=max(self._timeout, timeout + 5),
            )
            resp.raise_for_status()
            return normalize_execute_result(resp.json())
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

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get("/screenshot")
        resp.raise_for_status()
        from cua_sandbox.transport.base import convert_screenshot

        return convert_screenshot(resp.content, format, quality)

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post("/screen_size")
        resp.raise_for_status()
        data = resp.json()
        return {"width": int(data["width"]), "height": int(data["height"])}

    async def get_environment(self) -> str:
        return "linux"
