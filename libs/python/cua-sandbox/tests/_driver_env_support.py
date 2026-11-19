"""Shared helpers for the ``sb.driver`` tests.

``sb.driver`` rides cua-spacesd's ``/mcp`` through the cua SDK's env client
(``SpacesdClient.http``). These tests never reach a guest: ``McpTransport`` is a
synthetic MCP receiver, and ``EnvHttp`` is a stand-in ``cua.SpacesdClient``
whose ``http`` is served by it (or by a canned reply). Import the fixtures into
a test module the way the other driver tests do (``native = _native_fixture``).
"""

import asyncio
import json
import sys
from types import SimpleNamespace

import httpx
import pytest
from cua_sandbox import _sdk
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.env import EnvTransport


class ChannelError(Exception):
    pass


class CanonicalDriver:
    def __init__(self, channel):
        self.channel = channel
        self.shutdowns = 0

    async def shutdown(self):
        self.shutdowns += 1
        await self.channel.close()


@pytest.fixture
def native(monkeypatch):
    """A fake ``cua_driver`` module, for the tests that must not load the real one."""
    sdk = SimpleNamespace(
        CuaDriver=CanonicalDriver,
        ForeignDriverEnvelopeChannel=object,
        ForeignDriverChannelError=SimpleNamespace(Failed=ChannelError),
        ForeignDriverChannelCapabilities=SimpleNamespace,
        ForeignDriverChannelIdentity=SimpleNamespace,
        ForeignDriverResponseEnvelope=SimpleNamespace,
        connect_remote_channel=CanonicalDriver,
        _native=SimpleNamespace(uniffi_set_event_loop=lambda loop: None),
    )
    monkeypatch.setitem(sys.modules, "cua_driver", sdk)
    return sdk


@pytest.fixture
def shared(native):
    """``native`` plus a fake shared MCP channel; yields ``(native, channels)``."""
    channels = []

    class Channel:
        def __init__(self, transport, principal):
            self.transport, self.principal = transport, principal
            self.opened, self.closes = False, 0
            self.canonical = native.CuaDriver(self)
            channels.append(self)

        async def open(self):
            self.opened = True

        def driver(self):
            assert self.opened
            return self.canonical

        def public_session(self):
            return "host-selected"

        async def close(self):
            self.closes += 1

    native.DriverServiceTransport = object
    native.DriverServiceHeader = SimpleNamespace
    native.DriverServiceResponse = SimpleNamespace
    native.DriverServiceTransportError = SimpleNamespace(Failed=RuntimeError)
    native.open_mcp_driver_channel = Channel
    return native, channels


@pytest.fixture
def records(monkeypatch):
    """The SDK's record types, without the native library."""
    types = SimpleNamespace(SpacesdHttpRequest=SimpleNamespace, SpacesdHttpHeader=SimpleNamespace)
    monkeypatch.setattr(_sdk, "native", lambda: types)
    return types


def negotiation():
    return dict(
        connection_id="connection-1",
        generation="generation-1",
        public_session="session-1",
        capabilities=dict(
            minimum_envelope_version=1, maximum_envelope_version=1, supports_cancellation=True
        ),
    )


class McpTransport:
    """A synthetic MCP receiver speaking the typed-envelope extension."""

    def __init__(self, *, sse=False, supported=True):
        self.events = []
        self.open_data = negotiation()
        self.response_data = dict(
            envelope_version=1,
            request_id="request-1",
            ok=True,
            result={"width": 1280},
            completion_known=True,
        )
        self.sse = sse
        self.supported = supported
        self.sessions = set()
        self.sequence = 0
        self.mutate = None
        self.fail_exchange = False
        self.exchange_started = asyncio.Event()
        self.exchange_wait = None
        self.cancel_releases_exchange = True
        self.exchange_aborted = False
        self.exchange_finished = asyncio.Event()
        self.cancel_received = asyncio.Event()
        self.exchange_timeouts = []

    async def request_service(
        self,
        name,
        *,
        method,
        path,
        body=None,
        json_body=None,
        headers=None,
        timeout=None,
        max_response_bytes=None,
    ):
        assert max_response_bytes == 16 * 1024 * 1024
        if body:
            json_body = json.loads(body)
        headers = httpx.Headers(headers)
        self.events.append((name, method, path, json_body, headers))
        assert name == "mcp" and path == "/mcp"
        assert headers["Accept"] == "application/json, text/event-stream"
        rpc = json_body.get("method") if json_body else None
        response_headers = {}
        if rpc == "initialize":
            assert "Mcp-Session-Id" not in headers
            self.sequence += 1
            session = f"http-session-{self.sequence}"
            self.sessions.add(session)
            response_headers["Mcp-Session-Id"] = session
            result = {
                "protocolVersion": "2025-06-18",
                "capabilities": {
                    "experimental": (
                        {"ai.cua.driver.envelopes": {"version": 1}} if self.supported else {}
                    )
                },
            }
        else:
            session = headers["Mcp-Session-Id"]
            assert headers["MCP-Protocol-Version"] == "2025-06-18"
            if session not in self.sessions:
                return httpx.Response(404)
            if method == "DELETE":
                self.sessions.remove(session)
                return httpx.Response(200)
            if rpc == "notifications/initialized":
                assert "id" not in json_body
                return httpx.Response(202)
            if rpc == "cua/driver/v1/open":
                result = dict(self.open_data, connection_id=session, generation=f"gen-{session}")
            else:
                assert json_body["params"]["connection_id"] == session
                assert json_body["params"]["generation"] == f"gen-{session}"
                if rpc == "cua/driver/v1/exchange":
                    self.exchange_timeouts.append(timeout)
                    self.exchange_started.set()
                    if self.exchange_wait is not None:
                        try:
                            await self.exchange_wait.wait()
                        except asyncio.CancelledError:
                            self.exchange_aborted = True
                            raise
                    if self.fail_exchange:
                        raise RuntimeError("sensitive transport diagnostic")
                    request = json_body["params"]["envelope"]
                    result = dict(self.response_data, request_id=request["request_id"])
                    self.exchange_finished.set()
                else:
                    assert rpc in ("cua/driver/v1/cancel", "cua/driver/v1/close")
                    if rpc == "cua/driver/v1/cancel":
                        self.cancel_received.set()
                        if self.cancel_releases_exchange and self.exchange_wait is not None:
                            self.exchange_wait.set()
                    result = {"ok": True}
        response = {"jsonrpc": "2.0", "id": json_body["id"], "result": result}
        if self.mutate:
            response = self.mutate(rpc, response)
        if self.sse:
            response_headers["Content-Type"] = "text/event-stream"
            return httpx.Response(
                200,
                content=("event: message\r\ndata: " + json.dumps(response) + "\r\n\r\n"),
                headers=response_headers,
            )
        return httpx.Response(200, json=response, headers=response_headers)


class EnvHttp:
    """A stand-in ``cua.SpacesdClient`` whose ``http`` is served by ``McpTransport``'s
    synthetic MCP receiver (or a canned reply)."""

    def __init__(self, mcp=None, reply=None):
        self.mcp = mcp
        self.reply = reply
        self.requests = []

    async def http(self, request):
        self.requests.append(request)
        if self.reply is not None:
            return self.reply
        response = await self.mcp.request_service(
            "mcp",
            method=request.method,
            path=request.path,
            body=bytes(request.body),
            headers=[(h.name, h.value) for h in request.headers],
            timeout=None if request.timeout_ms is None else request.timeout_ms / 1000,
            max_response_bytes=request.max_response_bytes,
        )
        return SimpleNamespace(
            status=response.status_code,
            headers=[SimpleNamespace(name=k, value=v) for k, v in response.headers.multi_items()],
            body=response.content,
        )


async def env_sandbox(env):
    """A connected sandbox whose cua-spacesd client is ``env``."""

    async def factory():
        return env

    sb = Sandbox(EnvTransport(env_factory=factory), _telemetry_enabled=False)
    await sb._connect()
    return sb


async def sandbox(mcp):
    """A connected sandbox whose cua-spacesd ``/mcp`` is the synthetic receiver ``mcp``."""
    return await env_sandbox(EnvHttp(mcp=mcp))
