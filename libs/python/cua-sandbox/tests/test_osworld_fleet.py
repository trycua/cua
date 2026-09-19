"""OSWorld disks on Fleet: ``agent_type="osworld"`` selects the OSWorld server."""

import json

import pytest
from cua_sandbox import Image
from cua_sandbox.pool import _ClaimHandle
from cua_sandbox.transport.fleet import (
    FleetTransport,
    OSWorldFleetTransport,
    fleet_transport_for,
)
from cua_sandbox.transport.fleet_cloud import (
    FleetCloudTransport,
    OSWorldFleetCloudTransport,
    default_server_port,
    fleet_cloud_transport_for,
)
from cua_sandbox.transport.osworld import normalize_execute_result
from fleet_sdk import HttpResponse, Sandbox

REF = "registry.example/osworld@sha256:" + "0" * 64


class FakeSDK:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    async def service_request(self, sandbox, service, path, request):
        self.calls.append((service, path, request))
        return self.responses.pop(0)


def response(status=200, body=b"{}", headers=()):
    return HttpResponse(status=status, headers=list(headers), body=body)


def bound():
    return Sandbox(namespace="demo", claim="claim", name="sb", services=["server", "port-3000"])


def test_from_registry_carries_agent_type_and_round_trips():
    image = Image.from_registry(REF, os_type="linux", kind="vm", agent_type="osworld")
    assert image._agent_type == "osworld"
    assert Image.from_dict(image.to_dict())._agent_type == "osworld"
    assert "agent_type" not in Image.from_registry(REF).to_dict()


def test_osworld_images_default_to_the_flask_port():
    osworld = Image.from_registry(REF, os_type="linux", kind="vm", agent_type="osworld")
    plain = Image.from_registry(REF, os_type="linux", kind="vm")
    assert default_server_port(osworld) == 5000
    assert default_server_port(plain) == 8000
    assert default_server_port(osworld, 5555) == 5555
    assert default_server_port(None) == 8000


def test_transport_classes_follow_agent_type():
    osworld = Image.from_registry(REF, os_type="linux", kind="vm", agent_type="osworld")
    assert fleet_transport_for("osworld") is OSWorldFleetTransport
    assert fleet_transport_for(None) is FleetTransport
    assert fleet_cloud_transport_for(osworld) is OSWorldFleetCloudTransport
    assert fleet_cloud_transport_for(Image.from_registry(REF)) is FleetCloudTransport
    assert fleet_cloud_transport_for(None) is FleetCloudTransport


def test_claim_handle_serializes_agent_type():
    handle = _ClaimHandle(namespace="ns", name="claim", agent_type="osworld")
    restored = _ClaimHandle.from_dict(handle.to_dict())
    assert restored.agent_type == "osworld"
    assert (
        _ClaimHandle.from_dict(
            {"version": 1, "provider": "fleet", "namespace": "ns", "claim": "c", "pool": "ns"}
        ).agent_type
        is None
    )


def test_execute_result_normalization():
    assert normalize_execute_result(
        {"status": "success", "output": "hi\n", "error": "", "returncode": 0}
    ) == {"stdout": "hi\n", "stderr": "", "returncode": 0}
    assert normalize_execute_result({"status": "error", "output": None, "error": "boom"}) == {
        "stdout": "",
        "stderr": "boom",
        "returncode": 1,
    }


@pytest.mark.asyncio
async def test_osworld_fleet_transport_speaks_the_flask_api():
    png = b"\x89PNG\r\n\x1a\n" + b"x" * 16
    sdk = FakeSDK(
        [
            response(body=png),
            response(body=b'{"width": 1920, "height": 1080}'),
            response(
                body=b'{"status": "success", "output": "user\\n", "error": "", "returncode": 0}'
            ),
            response(body=b'{"AT": "<tree/>"}'),
        ]
    )
    transport = OSWorldFleetTransport(sdk=sdk, bound=bound(), service_name="server")
    await transport.connect()

    assert await transport.screenshot() == png
    assert await transport.get_screen_size() == {"width": 1920, "height": 1080}
    result = await transport.send("run_command", command="whoami", timeout=10)
    assert result == {"stdout": "user\n", "stderr": "", "returncode": 0}
    assert await transport.send("accessibility") == {"AT": "<tree/>"}
    assert await transport.get_environment() == "linux"

    services_and_paths = [(service, path, request.method) for service, path, request in sdk.calls]
    assert services_and_paths == [
        ("server", "/screenshot", "GET"),
        ("server", "/screen_size", "POST"),
        ("server", "/execute", "POST"),
        ("server", "/accessibility", "GET"),
    ]
    assert json.loads(sdk.calls[2][2].body) == {"command": "whoami", "shell": True}
    with pytest.raises(ValueError):
        await transport.send("mouse.click", x=1, y=2)
