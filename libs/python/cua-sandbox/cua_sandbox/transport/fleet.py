"""Computer-server transport routed through Cyclops named services."""

from __future__ import annotations

import asyncio
import json
import math
from typing import Any, Dict, List, Optional

import httpx
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.computer_server import (
    decode_screenshot_response,
    normalize_screen_size,
    parse_command_response,
)
from fleet_sdk import HttpHeader, HttpRequest, HttpRequestBuilder

_CMD_MAX_RETRIES = 3
_CMD_RETRY_BACKOFF_S = 0.5


def build_http_request(
    *,
    method: str,
    url: str,
    headers: Optional[List[Any]] = None,
    body: Optional[bytes] = None,
    timeout_secs: Optional[int] = None,
) -> HttpRequest:
    """Construct ``fleet_sdk.HttpRequest`` through the builder API.

    The builder treats optional record fields as skippable, so request
    construction keeps working when the Fleet SDK adds fields. An absent
    ``timeout_secs`` falls back to the native client's 30-second default.
    """
    builder = HttpRequestBuilder().method(method).url(url).headers(headers or [])
    if body is not None:
        builder = builder.body(body)
    if timeout_secs is not None:
        builder = builder.timeout_secs(timeout_secs)
    return builder.build()


def _whole_seconds(timeout: Optional[float]) -> Optional[int]:
    if timeout is None or timeout <= 0:
        return None
    return math.ceil(timeout)


class FleetTransport(Transport):
    """Route computer-server requests through ``CyclopsClient.service_request``."""

    def __init__(
        self,
        *,
        sdk: Any,
        bound: Any,
        service_name: str = "api",
        timeout: float = 30.0,
        owns_sdk: bool = False,
        **_: Any,
    ) -> None:
        self._sdk = sdk
        self._bound = bound
        self._service_name = service_name
        self._timeout = timeout
        self._owns_sdk = owns_sdk
        self._connected = False
        self._sdk_closed = False

    async def connect(self) -> None:
        if self._service_name not in self._bound.services:
            raise ValueError(f"Fleet sandbox does not expose service {self._service_name!r}")
        self._connected = True

    async def disconnect(self) -> None:
        self._connected = False
        # A transport constructed with owns_sdk=True (e.g. by Lease.wait) is the
        # sole holder of its Fleet client, so disconnect is where that client's
        # HTTP resources are returned.
        if self._owns_sdk and not self._sdk_closed:
            await self._sdk.close()
            self._sdk_closed = True

    async def request_service(
        self,
        name: str,
        *,
        method: str,
        path: str,
        json_body: Any = None,
        headers: dict[str, str] | None = None,
    ) -> httpx.Response:
        if name not in self._bound.services:
            raise ValueError(f"Fleet sandbox does not expose service {name!r}")
        return await self._request(
            method, path, json_body=json_body, service_name=name, extra_headers=headers
        )

    async def _request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        service_name: str | None = None,
        extra_headers: dict[str, str] | None = None,
    ) -> httpx.Response:
        assert self._connected, "Transport not connected"
        body = None if json_body is None else json.dumps(json_body).encode()
        headers = (
            [] if body is None else [HttpHeader(name="content-type", value="application/json")]
        )
        for name, value in (extra_headers or {}).items():
            headers.append(HttpHeader(name=name, value=value))
        result = await self._sdk.service_request(
            self._bound,
            service_name or self._service_name,
            path,
            build_http_request(
                method=method,
                url=f"https://service.invalid{path}",
                headers=headers,
                body=body,
                timeout_secs=_whole_seconds(self._timeout),
            ),
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
