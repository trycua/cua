"""Computer-server transport routed through Cyclops named services."""

from __future__ import annotations

import asyncio
import threading
from typing import Any, Dict, Optional

import httpx

from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.computer_server import (
    decode_screenshot_response,
    normalize_screen_size,
    parse_command_response,
)

_CMD_MAX_RETRIES = 3
_CMD_RETRY_BACKOFF_S = 0.5


class FleetTransport(Transport):
    """Route computer-server requests through ``SDK.service_client``."""

    def __init__(
        self,
        *,
        sdk: Any,
        bound: Any,
        service_name: str = "api",
        timeout: float = 30.0,
        call_lock: Optional[threading.Lock] = None,
    ) -> None:
        self._sdk = sdk
        self._bound = bound
        self._service_name = service_name
        self._timeout = timeout
        self._call_lock = call_lock or threading.Lock()
        self._client: Any = None

    async def connect(self) -> None:
        if self._client is None:
            self._client = await self._run(
                self._sdk.service_client, self._bound, self._service_name
            )

    async def disconnect(self) -> None:
        if self._client is not None:
            await self._run(self._client.close)
            self._client = None

    async def _cmd(self, command: str, params: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        client = self._require_client()
        body: Dict[str, Any] = {"command": command}
        if params:
            body["params"] = params
        server_timeout = (params or {}).get("timeout")
        timeout = (
            httpx.Timeout(self._timeout, read=float(server_timeout) + 10)
            if server_timeout is not None
            else None
        )
        response = None
        for attempt in range(_CMD_MAX_RETRIES):
            response = await self._run(client.post, "/cmd", json=body, timeout=timeout)
            if response.status_code < 500 or attempt == _CMD_MAX_RETRIES - 1:
                break
            await asyncio.sleep(_CMD_RETRY_BACKOFF_S * (2**attempt))
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
            response = await self._run(self._require_client().get, "/status")
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
        response = await self._run(self._require_client().post, "/pty", json=body)
        response.raise_for_status()
        return response.json()

    async def pty_send(self, pid: int, data: str) -> None:
        response = await self._run(
            self._require_client().post, f"/pty/{pid}/stdin", json={"data": data}
        )
        response.raise_for_status()

    async def pty_kill(self, pid: int) -> bool:
        response = await self._run(self._require_client().delete, f"/pty/{pid}")
        response.raise_for_status()
        return bool(response.json().get("killed", True))

    async def pty_info(self, pid: int) -> Optional[Dict[str, Any]]:
        response = await self._run(self._require_client().get, f"/pty/{pid}")
        if response.status_code == 404:
            return None
        response.raise_for_status()
        return response.json()

    async def _run(self, operation: Any, *args: Any, **kwargs: Any) -> Any:
        return await asyncio.to_thread(self._call_locked, operation, args, kwargs)

    def _call_locked(self, operation: Any, args: tuple[Any, ...], kwargs: Dict[str, Any]) -> Any:
        with self._call_lock:
            return operation(*args, **kwargs)

    def _require_client(self) -> Any:
        assert self._client is not None, "Transport not connected"
        return self._client
