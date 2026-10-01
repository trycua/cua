"""FleetTransport: named services through the Fleet gateway, interfaces
through cua-spacesd on the claim's ``env`` service (the cua SDK).

The computer-server wire tests that lived here (``POST /cmd`` bodies, the
``/pty`` routes, "connect rejects a missing service") were replaced: the
interfaces no longer ride ``service_request``, and connecting to a bound
claim is daemon-agnostic.
"""

from types import SimpleNamespace

import cua_sandbox.transport.fleet as fleet_transport
import pytest
from cua_sandbox import Sandbox as CuaSandbox
from cua_sandbox import SpacesdNotAvailable
from cua_sandbox.transport import fleet as fleet_module
from cua_sandbox.transport.fleet import FleetTransport, build_http_request
from fleet_sdk import HttpHeader, HttpRequest, HttpResponse, Sandbox

from tests._fake_env import FakeEnv


class FakeSDK:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    async def service_request(self, sandbox, service, path, request):
        self.calls.append((sandbox, service, path, request))
        return self.responses.pop(0)


def response(status=200, body=b"{}"):
    return HttpResponse(status=status, headers=[], body=body)


def sandbox(services=("api",)):
    return Sandbox(
        namespace="demo", claim="claim-demo", name="sandbox-demo", services=list(services)
    )


class FakeFleetHandle:
    def __init__(self, env):
        self.env_calls = 0
        self._env = env

    async def spacesd(self, probe_timeout_ms):
        self.env_calls += 1
        return self._env


@pytest.mark.asyncio
async def test_interfaces_use_the_env_service_through_the_sdk(monkeypatch):
    env = FakeEnv()
    handle = FakeFleetHandle(env)
    attached = []

    async def fleet_sandbox(namespace, claim):
        attached.append((namespace, claim))
        return handle

    monkeypatch.setattr(fleet_module, "fleet_sandbox", fleet_sandbox)
    sdk = FakeSDK([])
    sb = CuaSandbox(FleetTransport(sdk=sdk, bound=sandbox(["env", "mcp"])), name="sandbox-demo")
    await sb._connect()

    result = await sb.shell.run("uname -a")
    png = await sb.screenshot()
    await sb.mouse.click(10, 20)

    assert attached == [("demo", "claim-demo")]
    assert handle.env_calls == 1, "one spacesd connection is reused"
    assert result.success and png.startswith(b"\x89PNG")
    assert ("click", 10.0, 20.0) in env.calls
    assert sdk.calls == [], "interfaces no longer ride Fleet service_request"


@pytest.mark.asyncio
async def test_connect_is_daemon_agnostic_and_interfaces_fail_clearly_without_env():
    sdk = FakeSDK([response(body=b'{"ok":true}')])
    transport = FleetTransport(sdk=sdk, bound=sandbox(["mcp"]), env_ready_timeout=0)
    sb = CuaSandbox(transport, name="sandbox-demo")
    await sb._connect()

    # Named services work on an image without cua-spacesd ...
    reply = await sb.services.request("mcp", method="POST", path="/mcp", json={"id": 1})
    assert reply.status_code == 200
    # ... and the interfaces say why they cannot.
    with pytest.raises(SpacesdNotAvailable, match="no 'env' service"):
        await sb.shell.run("true")


@pytest.mark.asyncio
async def test_unknown_named_service_is_rejected():
    transport = FleetTransport(sdk=FakeSDK([]), bound=sandbox(["env"]))
    await transport.connect()
    with pytest.raises(ValueError, match="does not expose service"):
        await transport.request_service("mcp", method="GET", path="/")


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
    sdk = FakeSDK([response(), response(), response()])
    transport = FleetTransport(sdk=sdk, bound=sandbox(), timeout=default_timeout)
    await transport.connect()

    await transport.request_service("api", method="POST", path="/exchange", timeout=119.25)
    await transport.request_service("api", method="GET", path="/status")
    # A later call without an override keeps the transport default (the
    # interfaces ride cua-spacesd, so only named services use this path).
    await transport.request_service("api", method="GET", path="/status")

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
