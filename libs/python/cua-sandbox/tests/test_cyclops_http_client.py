from __future__ import annotations

from typing import Any

import httpx
import pytest
from cua_sandbox.transport.cyclops_http_client import CyclopsHttpClient
from fleet_sdk import HttpError, HttpRequest


class RecordingClient:
    def __init__(self, response: httpx.Response | Exception) -> None:
        self.response = response
        self.calls: list[tuple[str, str, dict[str, Any]]] = []

    async def request(self, method: str, url: str, **kwargs: Any) -> httpx.Response:
        self.calls.append((method, url, kwargs))
        if isinstance(self.response, Exception):
            raise self.response
        return self.response


def fleet_request(timeout_secs: int | None) -> HttpRequest:
    return HttpRequest(
        method="POST",
        url="https://fleet.example/api/svc/ns/computer-server/cmd",
        headers=[],
        body=b"{}",
        timeout_secs=timeout_secs,
    )


@pytest.mark.asyncio
async def test_execute_forwards_per_request_timeout_to_httpx() -> None:
    response = httpx.Response(200, content=b"ok")
    client = RecordingClient(response)

    result = await CyclopsHttpClient(client).execute(fleet_request(120))

    assert result.status == 200
    assert client.calls == [
        (
            "POST",
            "https://fleet.example/api/svc/ns/computer-server/cmd",
            {"headers": {}, "content": b"{}", "timeout": 120},
        )
    ]


@pytest.mark.asyncio
async def test_execute_uses_shared_client_default_without_request_timeout() -> None:
    client = RecordingClient(httpx.Response(200, content=b"ok"))

    await CyclopsHttpClient(client).execute(fleet_request(None))

    assert "timeout" not in client.calls[0][2]


@pytest.mark.asyncio
async def test_execute_reports_transport_error_type_when_message_is_empty() -> None:
    client = RecordingClient(httpx.ReadTimeout(""))

    with pytest.raises(HttpError.Transport) as raised:
        await CyclopsHttpClient(client).execute(fleet_request(120))

    assert raised.value.reason == "ReadTimeout"
