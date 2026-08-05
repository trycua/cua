"""Unit tests for the httpx bridge behind the generated Cyclops SDK."""

import httpx
import pytest
from cua_sandbox._config import _global_config, configure
from cua_sandbox.transport.cyclops_http_client import CyclopsHttpClient


@pytest.fixture(autouse=True)
def _reset_fleet_request_timeout():
    _global_config.fleet_request_timeout = None
    yield
    _global_config.fleet_request_timeout = None


@pytest.mark.asyncio
async def test_default_client_uses_default_fleet_request_timeout():
    client = CyclopsHttpClient()
    try:
        assert client._client.timeout == httpx.Timeout(30.0)
    finally:
        await client.aclose()


@pytest.mark.asyncio
async def test_default_client_honors_configured_fleet_request_timeout():
    configure(fleet_request_timeout=600.0)
    client = CyclopsHttpClient()
    try:
        assert client._client.timeout == httpx.Timeout(600.0)
    finally:
        await client.aclose()


@pytest.mark.asyncio
async def test_injected_client_keeps_its_own_timeout():
    configure(fleet_request_timeout=600.0)
    injected = httpx.AsyncClient(timeout=5.0)
    client = CyclopsHttpClient(injected)
    try:
        assert client._client.timeout == httpx.Timeout(5.0)
    finally:
        await injected.aclose()
