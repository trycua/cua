"""httpx bridge for the generated Cyclops SDK callback interface."""

from __future__ import annotations

import httpx
from fleet_sdk import HttpClient, HttpError, HttpHeader, HttpRequest, HttpResponse


class CyclopsHttpClient(HttpClient):
    """Execute SDK requests with one shared async httpx client."""

    def __init__(self, client: httpx.AsyncClient | None = None) -> None:
        self._client = client or httpx.AsyncClient(timeout=30.0)
        self._owns_client = client is None

    async def execute(self, request: HttpRequest) -> HttpResponse:
        request_kwargs = {
            "headers": {header.name: header.value for header in request.headers},
            "content": request.body,
        }
        if request.timeout_secs is not None:
            request_kwargs["timeout"] = request.timeout_secs

        try:
            response = await self._client.request(
                request.method,
                request.url,
                **request_kwargs,
            )
        except httpx.TransportError as error:
            raise HttpError.Transport(str(error) or type(error).__name__) from error
        return HttpResponse(
            status=response.status_code,
            headers=[
                HttpHeader(name=name, value=value) for name, value in response.headers.multi_items()
            ],
            body=response.content,
        )

    async def aclose(self) -> None:
        if self._owns_client:
            await self._client.aclose()
