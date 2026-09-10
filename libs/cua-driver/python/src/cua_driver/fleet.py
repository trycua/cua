"""Optional byte adapter for a caller-owned Fleet client and live sandbox target.

Importing this module does not import Fleet. The caller retains the claim and
client, and must close the Driver channel before releasing either of them.
"""

from __future__ import annotations

import asyncio
import copy
import math
import uuid

from . import (
    DriverServiceHeader,
    DriverServiceResponse,
    DriverServiceTransport,
    DriverServiceTransportError,
    open_mcp_driver_channel,
)


def open_fleet_mcp_driver_channel(client, sandbox, *, service="mcp"):
    """Create a shared channel bound to an existing authenticated Fleet target.

    Await ``channel.open()``, use ``channel.driver()`` and
    ``channel.public_session()``, then await ``channel.close()`` before releasing
    the claim. This function never acquires credentials or releases Fleet state.
    """
    if not isinstance(service, str) or service not in sandbox.services:
        raise ValueError("Fleet sandbox does not expose the requested Driver service")
    if not callable(getattr(client, "service_request", None)):
        raise TypeError("Fleet client must provide service_request")
    from fleet_sdk import HttpHeader, HttpRequestBuilder

    loop = asyncio.get_running_loop()
    target = copy.deepcopy(sandbox)
    service_request = client.service_request

    class FleetServiceTransport(DriverServiceTransport):
        async def send(self, request):
            try:
                if asyncio.get_running_loop() is not loop:
                    raise RuntimeError
                http_request = (
                    HttpRequestBuilder()
                    .method(request.method)
                    .url("https://service.invalid" + request.path)
                    .headers(
                        [
                            HttpHeader(name=header.name, value=header.value)
                            for header in request.headers
                        ]
                    )
                    .body(request.body)
                    .timeout_secs(max(1, math.ceil(request.timeout_ms / 1000)))
                    .build()
                )
                response = await service_request(target, service, request.path, http_request)
                return DriverServiceResponse(
                    status=response.status,
                    headers=[
                        DriverServiceHeader(name=header.name, value=header.value)
                        for header in response.headers
                    ],
                    body=response.body,
                )
            except Exception:
                raise DriverServiceTransportError.Failed(
                    "Fleet Driver service transport failed; completion is unknown"
                ) from None

    return open_mcp_driver_channel(FleetServiceTransport(), uuid.uuid4().hex)
