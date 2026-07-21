import base64
import json

import pytest
from cyclops_sdk import HttpResponse, Sandbox
from cua_sandbox.transport.fleet import FleetTransport


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
