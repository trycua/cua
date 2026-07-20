from __future__ import annotations

from unittest.mock import AsyncMock, patch

import pytest

from cua_sandbox import Image, Sandbox
from cua_sandbox.providers.base import ProvisionedSandbox


class FakeProvider:
    def __init__(self, transport):
        self.transport = transport
        self.create_calls = []
        self.destroy_calls = []
        self.handle = object()

    async def create(self, image, **kwargs):
        self.create_calls.append((image, kwargs))
        return ProvisionedSandbox(name=kwargs["name"], transport=self.transport, handle=self.handle)

    async def destroy(self, handle):
        self.destroy_calls.append(handle)


def make_transport():
    transport = AsyncMock()
    transport.get_environment.return_value = "linux"
    return transport


@pytest.mark.asyncio
async def test_create_uses_explicit_provider_instead_of_cloud_transport():
    transport = make_transport()
    provider = FakeProvider(transport)

    with patch("cua_sandbox.sandbox.CloudTransport", side_effect=AssertionError("unexpected cloud")):
        sandbox = await Sandbox.create(
            Image.linux(),
            name="fleet-demo",
            provider=provider,
            cpu=8,
            memory_mb=8192,
            request_timeout=60,
            telemetry_enabled=False,
        )

    assert sandbox.name == "fleet-demo"
    transport.connect.assert_awaited_once()
    assert len(provider.create_calls) == 1
    image, kwargs = provider.create_calls[0]
    assert image.os_type == "linux"
    assert kwargs == {
        "name": "fleet-demo",
        "cpu": 8,
        "memory_mb": 8192,
        "disk_gb": None,
        "region": "us-east-1",
        "time_to_start": None,
        "request_timeout": 60,
    }


@pytest.mark.asyncio
async def test_destroy_disconnects_then_deletes_provider_resources():
    events = []
    transport = make_transport()

    async def disconnect():
        events.append("disconnect")

    transport.disconnect.side_effect = disconnect
    provider = FakeProvider(transport)

    async def destroy(handle):
        assert handle is provider.handle
        events.append("destroy")

    provider.destroy = destroy
    sandbox = await Sandbox.create(
        Image.linux(), name="fleet-demo", provider=provider, telemetry_enabled=False
    )

    await sandbox.destroy()

    assert events == ["disconnect", "destroy"]


@pytest.mark.asyncio
async def test_ephemeral_destroys_provider_resources_on_normal_exit():
    transport = make_transport()
    provider = FakeProvider(transport)

    async with Sandbox.ephemeral(
        Image.linux(), name="fleet-demo", provider=provider, telemetry_enabled=False
    ):
        pass

    assert provider.destroy_calls == [provider.handle]


@pytest.mark.asyncio
async def test_ephemeral_destroys_provider_resources_on_error_exit():
    transport = make_transport()
    provider = FakeProvider(transport)

    with pytest.raises(RuntimeError, match="boom"):
        async with Sandbox.ephemeral(
            Image.linux(), name="fleet-demo", provider=provider, telemetry_enabled=False
        ):
            raise RuntimeError("boom")

    assert provider.destroy_calls == [provider.handle]


@pytest.mark.asyncio
async def test_provider_creation_gets_generated_name():
    transport = make_transport()
    provider = FakeProvider(transport)

    sandbox = await Sandbox.create(Image.linux(), provider=provider, telemetry_enabled=False)

    assert sandbox.name
    assert provider.create_calls[0][1]["name"] == sandbox.name

@pytest.mark.asyncio
async def test_connect_failure_deletes_provisioned_resources():
    transport = make_transport()
    transport.connect.side_effect = RuntimeError("connect failed")
    provider = FakeProvider(transport)

    with pytest.raises(RuntimeError, match="connect failed"):
        await Sandbox.create(
            Image.linux(), name="fleet-demo", provider=provider, telemetry_enabled=False
        )

    assert provider.destroy_calls == [provider.handle]
