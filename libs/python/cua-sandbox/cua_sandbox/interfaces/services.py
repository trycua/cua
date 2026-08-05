"""Named service interface for sandboxes that expose auxiliary endpoints."""

from __future__ import annotations

from typing import Any

from cua_sandbox.transport.base import Transport


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

        Fleet routes the request through the authenticated lease. Other
        transports raise :class:`NotImplementedError` unless they expose named
        services as well.
        """
        return await self._transport.request_service(
            name, method=method, path=path, json_body=json, headers=headers
        )
