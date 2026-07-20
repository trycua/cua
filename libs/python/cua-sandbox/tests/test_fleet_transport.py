from __future__ import annotations

import base64

import httpx
import pytest

from cua_sandbox.transport.fleet import FleetTransport


class FakeResponse:
    def __init__(self, *, status_code=200, text="", payload=None):
        self.status_code = status_code
        self.text = text
        self._payload = payload

    def raise_for_status(self):
        if self.status_code >= 400:
            request = httpx.Request("GET", "https://cyclops.invalid/test")
            response = httpx.Response(self.status_code, request=request)
            raise httpx.HTTPStatusError("failed", request=request, response=response)

    def json(self):
        return self._payload


class FakeServiceClient:
    def __init__(self):
        self.calls = []
        self.responses = []
        self.closed = False

    def queue(self, response):
        self.responses.append(response)

    def _response(self):
        return self.responses.pop(0)

    def post(self, path, **kwargs):
        self.calls.append(("post", path, kwargs))
        return self._response()

    def get(self, path, **kwargs):
        self.calls.append(("get", path, kwargs))
        return self._response()

    def delete(self, path, **kwargs):
        self.calls.append(("delete", path, kwargs))
        return self._response()

    def close(self):
        self.closed = True


class FakeSDK:
    def __init__(self, client):
        self.client = client
        self.calls = []

    def service_client(self, bound, service):
        self.calls.append((bound, service))
        return self.client


BOUND = {
    "namespace": "demo",
    "claim": "claim-demo",
    "sandbox": "sandbox-demo",
    "services": ["api"],
}


@pytest.mark.asyncio
async def test_connect_selects_named_api_service():
    client = FakeServiceClient()
    sdk = FakeSDK(client)
    transport = FleetTransport(sdk=sdk, bound=BOUND, service_name="api")

    await transport.connect()

    assert sdk.calls == [(BOUND, "api")]


@pytest.mark.asyncio
async def test_send_posts_computer_server_command():
    client = FakeServiceClient()
    client.queue(FakeResponse(text='data: {"success": true, "result": {"stdout": "ok"}}\n\n'))
    transport = FleetTransport(sdk=FakeSDK(client), bound=BOUND)
    await transport.connect()

    result = await transport.send("run_command", command="echo ok", timeout=10)

    assert result == {"stdout": "ok"}
    method, path, kwargs = client.calls[0]
    assert (method, path) == ("post", "/cmd")
    assert kwargs["json"] == {
        "command": "run_command",
        "params": {"command": "echo ok", "timeout": 10},
    }
    assert isinstance(kwargs["timeout"], httpx.Timeout)


@pytest.mark.asyncio
async def test_screenshot_and_screen_size_use_shared_codec():
    client = FakeServiceClient()
    encoded = base64.b64encode(b"png-data").decode()
    client.queue(FakeResponse(text=f'data: {{"success": true, "image_data": "{encoded}"}}\n\n'))
    client.queue(
        FakeResponse(text='data: {"success": true, "result": {"width": 1024, "height": 768}}\n\n')
    )
    transport = FleetTransport(sdk=FakeSDK(client), bound=BOUND)
    await transport.connect()

    assert await transport.screenshot() == b"png-data"
    assert await transport.get_screen_size() == {"width": 1024, "height": 768}


@pytest.mark.asyncio
async def test_environment_and_pty_routes():
    client = FakeServiceClient()
    client.queue(FakeResponse(payload={"os_type": "linux"}))
    client.queue(FakeResponse(payload={"pid": 42, "cols": 120, "rows": 40}))
    client.queue(FakeResponse(payload={"ok": True}))
    client.queue(FakeResponse(payload={"killed": True}))
    client.queue(FakeResponse(payload={"pid": 42}))
    transport = FleetTransport(sdk=FakeSDK(client), bound=BOUND)
    await transport.connect()

    assert await transport.get_environment() == "linux"
    assert await transport.pty_create(command="bash") == {"pid": 42, "cols": 120, "rows": 40}
    await transport.pty_send(42, "ls\n")
    assert await transport.pty_kill(42) is True
    assert await transport.pty_info(42) == {"pid": 42}
    assert [(method, path) for method, path, _ in client.calls] == [
        ("get", "/status"),
        ("post", "/pty"),
        ("post", "/pty/42/stdin"),
        ("delete", "/pty/42"),
        ("get", "/pty/42"),
    ]


@pytest.mark.asyncio
async def test_disconnect_closes_service_client():
    client = FakeServiceClient()
    transport = FleetTransport(sdk=FakeSDK(client), bound=BOUND)
    await transport.connect()

    await transport.disconnect()

    assert client.closed is True

@pytest.mark.asyncio
async def test_concurrent_requests_are_serialized_for_shared_sdk_runner():
    import threading
    import time

    class ConcurrentClient(FakeServiceClient):
        def __init__(self):
            super().__init__()
            self.active = 0
            self.max_active = 0
            self.guard = threading.Lock()

        def post(self, path, **kwargs):
            with self.guard:
                self.active += 1
                self.max_active = max(self.max_active, self.active)
            time.sleep(0.02)
            with self.guard:
                self.active -= 1
            return FakeResponse(text='data: {"success": true, "result": "ok"}\n\n')

    client = ConcurrentClient()
    transport = FleetTransport(sdk=FakeSDK(client), bound=BOUND)
    await transport.connect()

    first, second = await __import__("asyncio").gather(
        transport.send("first"), transport.send("second")
    )

    assert (first, second) == ("ok", "ok")
    assert client.max_active == 1
