from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import httpx
import pytest
from cua_sandbox.interfaces.services import ServiceHandle

pytestmark = pytest.mark.asyncio


@pytest.mark.parametrize(
    ("path", "normalized"),
    [("health", "/health"), ("/health", "/health"), ("", "/"), ("health?x=1", "/health?x=1")],
)
async def test_response_request_url_matches_forwarded_path(monkeypatch, path, normalized):
    native_service = SimpleNamespace(
        request=AsyncMock(return_value=SimpleNamespace(status=200, headers=[], body=b"ok"))
    )
    native_handle = SimpleNamespace(service=Mock(return_value=native_service))
    sandbox = SimpleNamespace(_native_handle=AsyncMock(return_value=native_handle))
    monkeypatch.setattr("cua_sandbox._sdk.native", lambda: SimpleNamespace())

    response = await ServiceHandle(sandbox, "web").request("get", path)

    assert native_service.request.await_args.args[1] == normalized
    assert str(response.request.url) == f"http://web.service{normalized}"


async def test_http_error_reports_forwarded_service_url(monkeypatch):
    native_service = SimpleNamespace(
        request=AsyncMock(return_value=SimpleNamespace(status=404, headers=[], body=b"missing"))
    )
    native_handle = SimpleNamespace(service=Mock(return_value=native_service))
    sandbox = SimpleNamespace(_native_handle=AsyncMock(return_value=native_handle))
    monkeypatch.setattr("cua_sandbox._sdk.native", lambda: SimpleNamespace())

    response = await ServiceHandle(sandbox, "web").request("get", "health")
    with pytest.raises(httpx.HTTPStatusError) as error:
        response.raise_for_status()
    assert str(error.value.request.url) == "http://web.service/health"
