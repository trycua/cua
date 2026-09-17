"""Fleet byte transport and ownership adapter for the shared Rust MCP client."""

from __future__ import annotations

import asyncio
import logging
from typing import Any

logger = logging.getLogger(__name__)

_FLEET_DRIVER_MAX_RESPONSE_BYTES = 16 * 1024 * 1024


def shared_channel(sdk: Any, transport: Any, service: str, principal: str) -> Any:
    from cua_sandbox.interfaces.driver import DriverConnectionError

    required = (
        "open_mcp_driver_channel",
        "DriverServiceTransport",
        "DriverServiceHeader",
        "DriverServiceResponse",
        "DriverServiceTransportError",
    )
    if not all(hasattr(sdk, name) for name in required):
        raise DriverConnectionError(
            "Install a cua-driver version providing open_mcp_driver_channel to use MCP"
        )
    loop = asyncio.get_running_loop()

    class ServiceTransport(sdk.DriverServiceTransport):
        async def send(self, request):
            try:
                if asyncio.get_running_loop() is not loop or not transport._connected:
                    raise RuntimeError
                response = await transport.request_service(
                    service,
                    method=request.method,
                    path=request.path,
                    body=request.body,
                    headers=[(header.name, header.value) for header in request.headers],
                    timeout=request.timeout_ms / 1000,
                    max_response_bytes=_FLEET_DRIVER_MAX_RESPONSE_BYTES,
                )
                if len(response.content) > _FLEET_DRIVER_MAX_RESPONSE_BYTES:
                    raise RuntimeError("Fleet Driver response exceeds the configured size limit")
                return sdk.DriverServiceResponse(
                    status=response.status_code,
                    headers=[
                        sdk.DriverServiceHeader(name=name, value=value)
                        for name, value in response.headers.multi_items()
                    ],
                    body=response.content,
                )
            except Exception:
                raise sdk.DriverServiceTransportError.Failed(
                    "Fleet Driver service transport failed; completion is unknown"
                ) from None

    class Connection:
        shared_mcp = True

        def __init__(self):
            self.closed = False
            self._transport = ServiceTransport()
            self._channel = sdk.open_mcp_driver_channel(self._transport, principal)

        @property
        def public_session(self):
            return self._channel.public_session()

        async def open(self):
            await self._channel.open()

        def driver(self):
            return self._channel.driver()

        async def close(self):
            self.closed = True
            try:
                await self._channel.close()
            except Exception:
                logger.warning("Driver cleanup failed; remote session cleanup is unconfirmed")

    return Connection()
