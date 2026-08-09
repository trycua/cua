"""Integration tests — cloud sandbox via CUA API.

    CUA_API_KEY=sk-... pytest tests/test_cloud.py -v -s

Requires a running cloud VM. Set CUA_TEST_CLOUD_VM_NAME to the VM name.
"""

from __future__ import annotations

import importlib
import os

import pytest
from cua_sandbox import Image, Sandbox
from cua_sandbox import _config
from cua_sandbox.runtime.base import RuntimeInfo

sandbox_module = importlib.import_module("cua_sandbox.sandbox")

pytestmark = pytest.mark.asyncio

API_KEY = os.environ.get("CUA_API_KEY")
VM_NAME = os.environ.get("CUA_TEST_CLOUD_VM_NAME", "steady-bluebird")

skip_no_key = pytest.mark.skipif(not API_KEY, reason="CUA_API_KEY not set")


@pytest.mark.parametrize(
    "auth_environment",
    [
        {"FLEETS_TOKEN": "fleet-token"},
        {"CUA_CLIENT_ID": "client-id", "CUA_CLIENT_SECRET": "client-secret"},
    ],
    ids=["workload-token", "client-credentials"],
)
async def test_cloud_routes_fleet_auth_without_explicit_legacy_key(monkeypatch, auth_environment):
    routes = []
    monkeypatch.setattr(_config, "_global_config", _config._Config())
    for variable in ("FLEETS_TOKEN", "CUA_CLIENT_ID", "CUA_CLIENT_SECRET"):
        monkeypatch.delenv(variable, raising=False)
    for variable, value in auth_environment.items():
        monkeypatch.setenv(variable, value)

    class FleetTransport:
        def __init__(self, **kwargs):
            routes.append(("fleet", kwargs))
            self.name = kwargs["name"]

        async def connect(self):
            return None

        async def disconnect(self):
            return None

        async def delete_vm(self):
            return None

    class LegacyTransport:
        def __init__(self, **kwargs):
            routes.append(("legacy", kwargs))
            self.name = kwargs["name"]

        async def connect(self):
            return None

        async def disconnect(self):
            return None

    monkeypatch.setattr(sandbox_module, "FleetCloudTransport", FleetTransport)
    monkeypatch.setattr(sandbox_module, "_make_transport", lambda **kwargs: LegacyTransport(**kwargs))

    fleet_sandbox = await Sandbox.create(Image.from_registry("example:latest"), name="fleet-demo")
    legacy_sandbox = await Sandbox.create(
        Image.from_registry("example:latest"), name="legacy-demo", api_key="sk-explicit"
    )
    await fleet_sandbox.disconnect()
    await legacy_sandbox.disconnect()

    assert Sandbox._uses_fleet(None)
    assert not Sandbox._uses_fleet("sk-explicit")
    assert [(route, values["name"]) for route, values in routes] == [
        ("fleet", "fleet-demo"),
        ("legacy", "legacy-demo"),
    ]


async def test_cloud_local_creation_never_routes_to_fleet(monkeypatch):
    calls = []

    class Runtime:
        async def start(self, image, name):
            calls.append(("start", image, name))
            return RuntimeInfo(host="127.0.0.1", api_port=8000, name=name, environment="linux")

    class FleetTransport:
        def __init__(self, **kwargs):
            raise AssertionError("local creation must not use Fleet")

    monkeypatch.setattr(sandbox_module, "FleetCloudTransport", FleetTransport)

    sandbox = await Sandbox.create(Image.linux(), name="local-demo", local=True, runtime=Runtime())
    await sandbox.disconnect()

    assert calls[0][2] == "local-demo"


@skip_no_key
async def test_cloud_connect_by_name():
    """Connect to an existing cloud VM by name and take a screenshot."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    screenshot = await sb.screenshot()
    assert screenshot[:4] == b"\x89PNG"
    assert len(screenshot) > 1000
    await sb.disconnect()


@skip_no_key
async def test_cloud_shell():
    """Run a shell command on a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    result = await sb.shell.run("echo hello-cloud")
    assert result.success
    assert "hello-cloud" in result.stdout
    await sb.disconnect()


@skip_no_key
async def test_cloud_screen_size():
    """Get screen dimensions from a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    w, h = await sb.get_dimensions()
    assert w > 0
    assert h > 0
    await sb.disconnect()


@skip_no_key
async def test_cloud_keyboard_mouse():
    """Basic keyboard and mouse operations on a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    await sb.mouse.move(100, 100)
    await sb.mouse.click(100, 100)
    await sb.keyboard.type("hello")
    await sb.disconnect()


@skip_no_key
async def test_cloud_environment():
    """Get environment info from a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    env = await sb.get_environment()
    assert env in ("windows", "mac", "linux", "browser")
    await sb.disconnect()


async def test_cloud_no_api_key_errors():
    """Connecting with no API key gives a clear error."""
    old = os.environ.pop("CUA_API_KEY", None)
    try:
        with pytest.raises(ValueError, match="No CUA API key found"):
            await Sandbox.connect("anything")
    finally:
        if old:
            os.environ["CUA_API_KEY"] = old


async def test_cloud_no_image_no_name_errors():
    """Creating without an image raises a clear error."""
    with pytest.raises((ValueError, TypeError)):
        await Sandbox._create(api_key="sk-test-fake-key")


@skip_no_key
async def test_cloud_ephemeral_linux():
    """Create an ephemeral cloud Linux VM, use it, and destroy on exit."""
    async with Sandbox.ephemeral(Image.linux(), api_key=API_KEY) as sb:
        assert sb.name is not None
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000
        result = await sb.shell.run("echo ephemeral-test")
        assert result.success
        assert "ephemeral-test" in result.stdout


@skip_no_key
async def test_cloud_ephemeral_android():
    """Create an ephemeral Android cloud VM, verify screenshot and display URL."""
    async with Sandbox.ephemeral(Image.android("14"), api_key=API_KEY) as sb:
        assert sb.name is not None
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000
        env = await sb.get_environment()
        assert env == "android"
        display_url = await sb.get_display_url(share=True)
        assert ".cua.sh" in display_url
        assert "password=" in display_url


@skip_no_key
async def test_cloud_invalid_api_key_errors():
    """An invalid (reversed) API key should get an HTTP error from the API."""
    reversed_key = API_KEY[::-1]
    with pytest.raises(Exception):
        sb = await Sandbox.connect(VM_NAME, api_key=reversed_key)
        await sb.screenshot()
        await sb.disconnect()
