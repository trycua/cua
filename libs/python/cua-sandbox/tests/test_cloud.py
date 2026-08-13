"""Integration tests — cloud sandbox via CUA API.

    CUA_API_KEY=sk-... pytest tests/test_cloud.py -v -s

Requires a running cloud VM. Set CUA_TEST_CLOUD_VM_NAME to the VM name.
"""

from __future__ import annotations

import importlib
import os
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from cua_sandbox import Image, Pool, Sandbox, _config
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

    class ClaimedSandbox:
        async def disconnect(self):
            return None

    class FleetPool:
        name = "fleet-pool"

        async def claim(self, **kwargs):
            routes.append(("fleet", kwargs))
            return ClaimedSandbox()

    async def apply_pool(cls, image, **kwargs):
        return FleetPool()

    class LegacyTransport:
        def __init__(self, **kwargs):
            routes.append(("legacy", kwargs))
            self.name = kwargs["name"]

        async def connect(self):
            return None

        async def disconnect(self):
            return None

    monkeypatch.setattr(Pool, "apply", classmethod(apply_pool))
    monkeypatch.setattr(
        sandbox_module, "_make_transport", lambda **kwargs: LegacyTransport(**kwargs)
    )

    fleet_sandbox = await Sandbox.create(Image.from_registry("example:latest"), name="fleet-demo")
    default_linux_sandbox = await Sandbox.create(Image.linux(), name="linux-demo")
    legacy_sandbox = await Sandbox.create(
        Image.from_registry("example:latest"), name="legacy-demo", api_key="sk-explicit"
    )
    await fleet_sandbox.disconnect()
    await default_linux_sandbox.disconnect()
    await legacy_sandbox.disconnect()

    assert Sandbox._uses_fleet(None)
    assert not Sandbox._uses_fleet("sk-explicit")
    assert [(route, values["name"]) for route, values in routes] == [
        ("fleet", "fleet-demo"),
        ("fleet", "linux-demo"),
        ("legacy", "legacy-demo"),
    ]


@pytest.mark.parametrize("legacy_source", ["configured", "environment", "credentials"])
async def test_registry_image_uses_legacy_auth_when_fleet_auth_is_unavailable(
    monkeypatch, legacy_source
):
    routes = []
    monkeypatch.setattr(_config, "_global_config", _config._Config())
    for variable in (
        "CUA_API_KEY",
        "FLEETS_TOKEN",
        "CUA_CLIENT_ID",
        "CUA_CLIENT_SECRET",
    ):
        monkeypatch.delenv(variable, raising=False)

    if legacy_source == "configured":
        _config.configure(api_key="sk-configured")
    elif legacy_source == "environment":
        monkeypatch.setenv("CUA_API_KEY", "sk-environment")
    else:
        monkeypatch.setattr(_config, "_read_credentials_key", lambda: "sk-credentials")

    class LegacyTransport:
        def __init__(self, **kwargs):
            routes.append(kwargs)
            self.name = kwargs["name"]

        async def connect(self):
            return None

        async def disconnect(self):
            return None

    async def reject_fleet_apply(cls, image, **kwargs):
        raise AssertionError("legacy authentication must not route registry images to Fleet")

    monkeypatch.setattr(Pool, "apply", classmethod(reject_fleet_apply))
    monkeypatch.setattr(
        sandbox_module, "_make_transport", lambda **kwargs: LegacyTransport(**kwargs)
    )

    sandbox = await Sandbox.create(Image.from_registry("example:latest"), name="legacy-demo")
    await sandbox.disconnect()

    assert not Sandbox._uses_fleet(None)
    assert [route["name"] for route in routes] == ["legacy-demo"]


async def test_ephemeral_registry_image_uses_legacy_auth_selector(monkeypatch):
    monkeypatch.setattr(_config, "_global_config", _config._Config(api_key="sk-configured"))
    for variable in ("FLEETS_TOKEN", "CUA_CLIENT_ID", "CUA_CLIENT_SECRET"):
        monkeypatch.delenv(variable, raising=False)

    legacy_sandbox = SimpleNamespace(
        _has_snapshots=False,
        name="legacy-demo",
        destroy=AsyncMock(),
    )
    create = AsyncMock()
    create_legacy = AsyncMock(return_value=legacy_sandbox)
    monkeypatch.setattr(Sandbox, "create", create)
    monkeypatch.setattr(Sandbox, "_create", create_legacy)

    async with Sandbox.ephemeral(Image.from_registry("example:latest"), name="legacy-demo"):
        pass

    create.assert_not_awaited()
    create_legacy.assert_awaited_once()
    legacy_sandbox.destroy.assert_awaited_once()


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


async def test_pool_backed_create_persists_claim_pool_mapping(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)

    class ClaimedSandbox:
        pass

    class FleetPool:
        name = "cua-cli-wif-smoke"

        async def claim(self, **kwargs):
            assert kwargs["name"] == "wif-smoke-123"
            return ClaimedSandbox()

    async def get_pool(cls, name):
        assert name == "cua-cli-wif-smoke"
        return FleetPool()

    monkeypatch.setattr(Pool, "get", classmethod(get_pool))

    await Sandbox.create(pool="cua-cli-wif-smoke", name="wif-smoke-123")

    assert sandbox_state.load("wif-smoke-123")["pool_name"] == "cua-cli-wif-smoke"


async def test_pool_backed_connect_loads_claim_pool_mapping(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    created = []
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    sandbox_state.save_fleet_claim("wif-smoke-123", "cua-cli-wif-smoke")
    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: True))

    class FleetTransport:
        def __init__(self, **kwargs):
            created.append(kwargs)
            self.name = kwargs["name"]

        async def connect(self):
            return None

        async def disconnect(self):
            return None

    monkeypatch.setattr(sandbox_module, "FleetCloudTransport", FleetTransport)

    await Sandbox.connect("wif-smoke-123")

    assert created[0]["name"] == "wif-smoke-123"
    assert created[0]["pool_name"] == "cua-cli-wif-smoke"


async def test_pool_backed_delete_removes_mapping_only_after_success(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    calls = []
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    sandbox_state.save_fleet_claim("wif-smoke-123", "cua-cli-wif-smoke")
    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: True))

    class FleetTransport:
        @classmethod
        async def delete_sandbox(cls, name, *, pool_name=None):
            calls.append((name, pool_name))

    monkeypatch.setattr(sandbox_module, "FleetCloudTransport", FleetTransport)

    await Sandbox.delete("wif-smoke-123")

    assert calls == [("wif-smoke-123", "cua-cli-wif-smoke")]
    assert sandbox_state.load("wif-smoke-123") is None


async def test_pool_backed_delete_keeps_mapping_when_claim_delete_fails(monkeypatch, tmp_path):
    from cua_sandbox import sandbox_state

    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    sandbox_state.save_fleet_claim("wif-smoke-123", "cua-cli-wif-smoke")
    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: True))

    class FleetTransport:
        @classmethod
        async def delete_sandbox(cls, name, *, pool_name=None):
            raise RuntimeError("claim delete failed")

    monkeypatch.setattr(sandbox_module, "FleetCloudTransport", FleetTransport)

    with pytest.raises(RuntimeError, match="claim delete failed"):
        await Sandbox.delete("wif-smoke-123")

    assert sandbox_state.load("wif-smoke-123")["pool_name"] == "cua-cli-wif-smoke"
