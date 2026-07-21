"""Computer-server transport routed through Cyclops named services."""

from __future__ import annotations

import asyncio
import json
from typing import Any, Dict, Optional

import httpx
from cyclops_sdk import HttpHeader, HttpRequest
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.computer_server import (
    decode_screenshot_response,
    normalize_screen_size,
    parse_command_response,
)

_CMD_MAX_RETRIES = 3
_CMD_RETRY_BACKOFF_S = 0.5


class FleetTransport(Transport):
    """Route computer-server requests through ``CyclopsClient.service_request``."""

    def __init__(
        self,
        *,
        sdk: Any,
        bound: Any,
        service_name: str = "api",
        timeout: float = 30.0,
        **_: Any,
    ) -> None:
        self._sdk = sdk
        self._bound = bound
        self._service_name = service_name
        self._timeout = timeout
        self._connected = False

    async def connect(self) -> None:
        if self._service_name not in self._bound.services:
            raise ValueError(f"Fleet sandbox does not expose service {self._service_name!r}")
        self._connected = True

    async def disconnect(self) -> None:
        self._connected = False

    async def _request(self, method: str, path: str, *, json_body: Any = None) -> httpx.Response:
        assert self._connected, "Transport not connected"
        body = None if json_body is None else json.dumps(json_body).encode()
        headers = [] if body is None else [HttpHeader(name="content-type", value="application/json")]
        result = await self._sdk.service_request(
            self._bound,
            self._service_name,
            path,
            HttpRequest(method=method, url=f"https://service.invalid{path}", headers=headers, body=body),
        )
        request = httpx.Request(method, f"https://service.invalid{path}")
        return httpx.Response(
            result.status,
            headers={header.name: header.value for header in result.headers},
            content=result.body,
            request=request,
        )

    async def _cmd(self, command: str, params: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        body: Dict[str, Any] = {"command": command}
        if params:
            body["params"] = params
        response = None
        for attempt in range(_CMD_MAX_RETRIES):
            response = await self._request("POST", "/cmd", json_body=body)
            if response.status_code < 500 or attempt == _CMD_MAX_RETRIES - 1:
                break
            await asyncio.sleep(_CMD_RETRY_BACKOFF_S * (2**attempt))
        assert response is not None
        response.raise_for_status()
        return parse_command_response(response.text)

    async def send(self, action: str, **params: Any) -> Any:
        result = await self._cmd(action, params if params else None)
        return result.get("result", result)

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        params = None if format == "png" else {"format": format, "quality": quality}
        return decode_screenshot_response(await self._cmd("screenshot", params))

    async def get_screen_size(self) -> Dict[str, int]:
        return normalize_screen_size(await self._cmd("get_screen_size"))

    async def get_environment(self) -> str:
        try:
            response = await self._request("GET", "/status")
            response.raise_for_status()
            payload = response.json()
            return payload.get("os_type", payload.get("platform", "linux"))
        except Exception:
            return "linux"

    async def pty_create(
        self,
        command: Optional[str] = None,
        cols: int = 120,
        rows: int = 40,
        cwd: Optional[str] = None,
        envs: Optional[Dict[str, str]] = None,
    ) -> Dict[str, Any]:
        body: Dict[str, Any] = {"cols": cols, "rows": rows}
        if command is not None:
            body["command"] = command
        if cwd is not None:
            body["cwd"] = cwd
        if envs is not None:
            body["envs"] = envs
        response = await self._request("POST", "/pty", json_body=body)
        response.raise_for_status()
        return response.json()

    async def pty_send(self, pid: int, data: str) -> None:
        response = await self._request("POST", f"/pty/{pid}/stdin", json_body={"data": data})
        response.raise_for_status()

    async def pty_kill(self, pid: int) -> bool:
        response = await self._request("DELETE", f"/pty/{pid}")
        response.raise_for_status()
        return bool(response.json().get("killed", True))

    async def pty_info(self, pid: int) -> Optional[Dict[str, Any]]:
        response = await self._request("GET", f"/pty/{pid}")
        if response.status_code == 404:
            return None
        response.raise_for_status()
        return response.json()
