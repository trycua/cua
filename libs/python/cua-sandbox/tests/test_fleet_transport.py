import base64
import json
from types import SimpleNamespace

import cua_sandbox.transport.fleet as fleet_transport
import pytest
from cua_sandbox.transport.fleet import FleetTransport, build_http_request
from fleet_sdk import HttpHeader, HttpRequest, HttpResponse, Sandbox


class FakeSDK:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    async def service_request(self, sandbox, service, path, request):
        self.calls.append((sandbox, service, path, request))
        return self.responses.pop(0)


def response(status=200, body=b"{}"):
    return HttpResponse(status=status, headers=[], body=body)


def sandbox():
    return Sandbox(namespace="demo", claim="claim-demo", name="sandbox-demo", services=["api"])


@pytest.mark.asyncio
async def test_service_request_forwards_command_json():
    sdk = FakeSDK([response(body=b'data: {"success":true,"result":"ok"}\n\n')])
    transport = FleetTransport(sdk=sdk, bound=sandbox())
    await transport.connect()

    assert await transport.send("shell.run", timeout=15) == "ok"
    _, service, path, request = sdk.calls[0]
    assert (service, path, request.method) == ("api", "/cmd", "POST")
    assert json.loads(request.body) == {"command": "shell.run", "params": {"timeout": 15}}
    assert request.timeout_secs == 30


@pytest.mark.asyncio
async def test_screenshot_and_pty_use_service_request():
    encoded = base64.b64encode(b"png-data").decode()
    sdk = FakeSDK(
        [
            response(body=f'data: {{"success":true,"image_data":"{encoded}"}}\n\n'.encode()),
            response(body=b'{"pid":42}'),
            response(body=b'{"killed":true}'),
        ]
    )
    transport = FleetTransport(sdk=sdk, bound=sandbox())
    await transport.connect()

    assert await transport.screenshot() == b"png-data"
    assert await transport.pty_create(command="bash") == {"pid": 42}
    assert await transport.pty_kill(42) is True
    assert [call[2] for call in sdk.calls] == ["/cmd", "/pty", "/pty/42"]


@pytest.mark.asyncio
async def test_connect_rejects_missing_service():
    transport = FleetTransport(
        sdk=FakeSDK([]),
        bound=Sandbox(namespace="demo", claim="claim", name="sandbox", services=[]),
    )
    with pytest.raises(ValueError, match="does not expose service"):
        await transport.connect()


def test_build_http_request_constructs_the_record_through_the_builder():
    unbounded = build_http_request(method="GET", url="https://service.invalid/status")

    assert isinstance(unbounded, HttpRequest)
    assert (unbounded.method, unbounded.headers, unbounded.body) == ("GET", [], None)
    assert unbounded.timeout_secs is None


def test_build_http_request_forwards_response_limit_to_the_builder(monkeypatch):
    class Builder:
        def __init__(self):
            self.request = SimpleNamespace(body=None, timeout_secs=None, max_response_bytes=None)

        def method(self, value):
            self.request.method = value
            return self

        def url(self, value):
            self.request.url = value
            return self

        def headers(self, value):
            self.request.headers = value
            return self

        def timeout_secs(self, value):
            self.request.timeout_secs = value
            return self

        def max_response_bytes(self, value):
            self.request.max_response_bytes = value
            return self

        def build(self):
            return self.request

    monkeypatch.setattr(fleet_transport, "HttpRequestBuilder", Builder)

    bounded = build_http_request(
        method="GET",
        url="https://service.invalid/status",
        timeout_secs=30,
        max_response_bytes=4096,
    )

    assert (bounded.method, bounded.headers, bounded.body) == ("GET", [], None)
    assert bounded.timeout_secs == 30
    assert bounded.max_response_bytes == 4096


@pytest.mark.asyncio
async def test_requests_are_bounded_by_the_transport_timeout():
    sdk = FakeSDK([response(), response()])
    transport = FleetTransport(sdk=sdk, bound=sandbox())
    await transport.connect()
    fractional = FleetTransport(sdk=sdk, bound=sandbox(), timeout=0.5)
    await fractional.connect()

    await transport.request_service("api", method="GET", path="/status")
    await fractional.request_service("api", method="GET", path="/status")

    assert sdk.calls[0][3].timeout_secs == 30
    assert sdk.calls[1][3].timeout_secs == 1


@pytest.mark.asyncio
async def test_service_response_limit_is_forwarded_and_exact_limit_is_accepted(monkeypatch):
    seen = []

    def build_request(**request):
        seen.append(request)
        return SimpleNamespace(**request)

    monkeypatch.setattr(fleet_transport, "build_http_request", build_request)
    sdk = FakeSDK([response(body=b"four")])
    transport = FleetTransport(sdk=sdk, bound=sandbox())
    await transport.connect()

    result = await transport.request_service(
        "api", method="GET", path="/status", max_response_bytes=4
    )

    assert seen[0]["max_response_bytes"] == 4
    assert result.content == b"four"


@pytest.mark.asyncio
async def test_service_response_over_limit_is_rejected_without_exposing_contents(monkeypatch):
    monkeypatch.setattr(
        fleet_transport, "build_http_request", lambda **request: SimpleNamespace(**request)
    )
    secret = b"private-response-fragment"
    sdk = FakeSDK([response(body=secret)])
    transport = FleetTransport(sdk=sdk, bound=sandbox())
    await transport.connect()

    with pytest.raises(RuntimeError, match="configured size limit") as error:
        await transport.request_service(
            "api", method="GET", path="/status", max_response_bytes=len(secret) - 1
        )

    assert secret.decode() not in str(error.value)


@pytest.mark.asyncio
@pytest.mark.parametrize("default_timeout", [30, 90])
async def test_service_timeout_override_does_not_change_existing_callers(default_timeout):
    sdk = FakeSDK([response(), response(), response(body=b'data: {"success":true}\n\n')])
    transport = FleetTransport(sdk=sdk, bound=sandbox(), timeout=default_timeout)
    await transport.connect()

    await transport.request_service("api", method="POST", path="/exchange", timeout=119.25)
    await transport.request_service("api", method="GET", path="/status")
    await transport.send("shell.run", timeout=15)

    assert [call[3].timeout_secs for call in sdk.calls] == [120, default_timeout, default_timeout]
    assert transport._timeout == default_timeout


@pytest.mark.asyncio
async def test_raw_service_bytes_and_duplicate_headers_are_preserved():
    sdk = FakeSDK(
        [
            HttpResponse(
                status=200,
                headers=[
                    HttpHeader(name="mcp-session-id", value="first"),
                    HttpHeader(name="mcp-session-id", value="second"),
                ],
                body=b"raw response",
            )
        ]
    )
    transport = FleetTransport(sdk=sdk, bound=sandbox())
    await transport.connect()
    result = await transport.request_service(
        "api",
        method="POST",
        path="/mcp",
        body=b"\x00raw bytes",
        headers=[("content-type", "application/octet-stream"), ("x-test", "a"), ("x-test", "b")],
    )
    request = sdk.calls[0][3]
    assert request.body == b"\x00raw bytes"
    assert [(h.name, h.value) for h in request.headers] == [
        ("content-type", "application/octet-stream"),
        ("x-test", "a"),
        ("x-test", "b"),
    ]
    assert result.content == b"raw response"
    assert result.headers.get_list("mcp-session-id") == ["first", "second"]


@pytest.mark.asyncio
async def test_raw_body_and_json_are_mutually_exclusive():
    sdk = FakeSDK([])
    transport = FleetTransport(sdk=sdk, bound=sandbox())
    await transport.connect()
    with pytest.raises(ValueError, match="either body or json_body"):
        await transport.request_service("api", method="POST", path="/mcp", body=b"", json_body={})
    assert not sdk.calls
