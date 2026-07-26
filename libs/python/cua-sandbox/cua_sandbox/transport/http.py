"""HTTPTransport — REST fallback for computer-server's POST /cmd endpoint.

The computer-server /cmd endpoint accepts JSON ``{"command": ..., "params": {...}}``
and returns an SSE stream with a single ``data: {...}`` frame containing the result.
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any, Dict, Optional

import httpx
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.computer_server import (
    decode_screenshot_response,
    normalize_screen_size,
    parse_command_response,
)

logger = logging.getLogger(__name__)

# Retry transient 5xx responses on /cmd. The computer-server can briefly
# return 5xx (e.g. when Traefik temporarily drops the pod from its
# endpoint list, or when the emulator's gRPC subsystem hangs during
# fork/exec). 4xx errors are not retried (client error, won't change).
# Read/transport timeouts are not retried either — the command may
# already be running on the server, and most /cmd actions aren't
# idempotent.
_CMD_MAX_RETRIES = 3
_CMD_RETRY_BACKOFF_S = 0.5  # doubled each retry: 0.5s, 1.0s, 2.0s


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
        """Send a command to POST /cmd and parse the SSE response.

        Retries on transient 5xx responses (server briefly unavailable,
        grpc fork hiccups, Traefik backend-unready). Does not retry on
        httpx exceptions — the request may have reached the server and
        started running a non-idempotent command.
        """
        assert self._client is not None, "Transport not connected"
        body = {"command": command}
        if params:
            body["params"] = params
        # When the caller passes a server-side timeout (e.g. push_timeout for
        # write_bytes, or timeout for run_command), the server may legitimately
        # take that long to respond.  Set the httpx read timeout to match so the
        # client doesn't drop the connection before the server finishes.
        server_timeout = (params or {}).get("timeout")
        if server_timeout is not None:
            # Add 10s headroom so the server timeout fires before the client one
            req_timeout = httpx.Timeout(self._timeout, read=float(server_timeout) + 10)
        else:
            req_timeout = None  # use client default

        resp: httpx.Response
        for attempt in range(_CMD_MAX_RETRIES):
            resp = await self._client.post("/cmd", json=body, timeout=req_timeout)
            if resp.status_code < 500 or attempt == _CMD_MAX_RETRIES - 1:
                break
            backoff = _CMD_RETRY_BACKOFF_S * (2**attempt)
            logger.debug(
                "[http] /cmd %s returned %d, retrying in %.1fs (attempt %d/%d)",
                command,
                resp.status_code,
                backoff,
                attempt + 1,
                _CMD_MAX_RETRIES,
            )
            await asyncio.sleep(backoff)

        resp.raise_for_status()
        return self._parse_sse(resp.text)

    _parse_sse = staticmethod(parse_command_response)

    async def send(self, action: str, **params: Any) -> Any:
        result = await self._cmd(action, params if params else None)
        return result.get("result", result)

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        params = None if format == "png" else {"format": format, "quality": quality}
        result = await self._cmd("screenshot", params)
        return decode_screenshot_response(result)

    async def get_screen_size(self) -> Dict[str, int]:
        result = await self._cmd("get_screen_size")
        return normalize_screen_size(result)

    # ── PTY over dedicated /pty_* routes ────────────────────────────────
    async def pty_create(
        self,
        command: Optional[str] = None,
        cols: int = 120,
        rows: int = 40,
        cwd: Optional[str] = None,
        envs: Optional[Dict[str, str]] = None,
    ) -> Dict[str, Any]:
        assert self._client is not None, "Transport not connected"
        body: Dict[str, Any] = {"cols": cols, "rows": rows}
        if command is not None:
            body["command"] = command
        if cwd is not None:
            body["cwd"] = cwd
        if envs is not None:
            body["envs"] = envs
        resp = await self._client.post("/pty", json=body)
        resp.raise_for_status()
        return resp.json()

    async def pty_send(self, pid: int, data: str) -> None:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post(f"/pty/{pid}/stdin", json={"data": data})
        resp.raise_for_status()

    async def pty_kill(self, pid: int) -> bool:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.delete(f"/pty/{pid}")
        resp.raise_for_status()
        return bool(resp.json().get("killed", True))

    async def pty_info(self, pid: int) -> Optional[Dict[str, Any]]:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get(f"/pty/{pid}")
        if resp.status_code == 404:
            return None
        resp.raise_for_status()
        return resp.json()

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
