"""Named services of a sandbox: requests, URLs and shareable URLs.

``sb.service(name)`` is the handle for a declared service; ``sb.services``
keeps the older request helper.
"""

from __future__ import annotations

import json as _json
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Any, Mapping, Optional, Sequence, Union

from cua_sandbox.transport.base import Transport

if TYPE_CHECKING:
    from cua_sandbox.options import PublicUrl
    from cua_sandbox.sandbox import Sandbox


class Services:
    """Request a named service exposed by the active sandbox connection."""

    def __init__(self, transport: Transport):
        self._transport = transport

    async def request(
        self,
        name: str,
        *,
        method: str,
        path: str,
        json: Any = None,
        headers: dict[str, str] | None = None,
    ) -> Any:
        """Send a request to a named service on this sandbox.

        Prefer ``sb.service(name).request(...)``.
        """
        return await self._transport.request_service(
            name, method=method, path=path, json_body=json, headers=headers
        )


Headers = Union[Mapping[str, str], Sequence[tuple[str, str]], None]


def _unix_to_iso(seconds: int) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")


class ServiceHandle:
    """One named service of a sandbox (``sb.service("mcp")``).

    Requests carry your headers, ``url()`` is usable from this machine and
    ``public_url()`` can be shared.
    """

    def __init__(self, sandbox: "Sandbox", name: str) -> None:
        self._sandbox = sandbox
        self.name = name

    def __repr__(self) -> str:
        return f"ServiceHandle({self.name!r})"

    async def _native(self) -> Any:
        return (await self._sandbox._native_handle()).service(self.name)

    async def request(
        self,
        method: str,
        path: str = "/",
        *,
        headers: Headers = None,
        body: bytes | str | None = None,
        json: Any = None,
        timeout: float = 30.0,
    ) -> Any:
        """One HTTP request to ``path`` on the service; returns an
        ``httpx.Response``."""
        import httpx
        from cua_sandbox._sdk import millis, native

        if body is not None and json is not None:
            raise ValueError("pass body= or json=, not both")
        pairs = list(headers.items()) if isinstance(headers, Mapping) else list(headers or [])
        if json is not None:
            body = _json.dumps(json).encode()
            if not any(k.lower() == "content-type" for k, _ in pairs):
                pairs.append(("content-type", "application/json"))
        if isinstance(body, str):
            body = body.encode()
        n = native()
        response = await (await self._native()).request(
            method.upper(),
            path if path.startswith("/") else f"/{path}",
            body,
            millis(timeout),
            [n.HttpHeader(name=k, value=v) for k, v in pairs],
        )
        return httpx.Response(
            response.status,
            headers=[(h.name, h.value) for h in response.headers],
            content=bytes(response.body),
            request=httpx.Request(method.upper(), f"http://{self.name}.service{path}"),
        )

    async def url(self) -> str:
        """A URL for the service usable from this machine with no credentials
        (no trailing slash): the published loopback port."""
        return str(await (await self._native()).url())

    async def public_url(self, ttl: float = 3600, *, label: Optional[str] = None) -> "PublicUrl":
        """A shareable URL that stops working after ``ttl`` seconds (60 s to
        24 h). See :meth:`cua_sandbox.Sandbox.public_url`."""
        return await self._sandbox.public_url(self.name, ttl=ttl, label=label)


def public_url_from_native(u: Any) -> "PublicUrl":
    from cua_sandbox.options import PublicUrl

    return PublicUrl(
        url=u.url,
        expires_at=_unix_to_iso(int(u.expires_at_unix)),
        id=u.id,
        service=u.service,
        provider_details=dict(u.provider_details),
    )
