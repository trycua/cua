"""Shared fixtures for cua-sandbox integration tests.

Each transport/runtime is exposed as a pytest fixture. Tests that need a
specific backend request the fixture by name; parametrized tests pull from
all available backends.

Environment variables control which backends are exercised:

    CUA_TEST_LOCAL=1              Enable localhost/local-sandbox tests (default: on)
    CUA_TEST_WS_URL=ws://...     Enable WebSocket transport tests
    CUA_TEST_HTTP_URL=http://...  Enable HTTP transport tests
    CUA_TEST_API_KEY=sk-...       API key for remote transports
    CUA_TEST_CONTAINER_NAME=...   Container name for HTTP cloud auth
"""

from __future__ import annotations

import os

import pytest
import pytest_asyncio
from cua_sandbox.localhost import Localhost
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.http import HTTPTransport
from cua_sandbox.transport.local import LocalTransport
from cua_sandbox.transport.websocket import WebSocketTransport

# ---------------------------------------------------------------------------
# Helper: read env config
# ---------------------------------------------------------------------------


def _env_bool(key: str, default: bool = False) -> bool:
    val = os.environ.get(key, "")
    if not val:
        return default
    return val.lower() in ("1", "true", "yes")


LOCAL_ENABLED = _env_bool("CUA_TEST_LOCAL", default=True)
WS_URL = os.environ.get("CUA_TEST_WS_URL")
HTTP_URL = os.environ.get("CUA_TEST_HTTP_URL")
API_KEY = os.environ.get("CUA_TEST_API_KEY")
CONTAINER_NAME = os.environ.get("CUA_TEST_CONTAINER_NAME")


# ---------------------------------------------------------------------------
# Transport fixtures
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def local_transport():
    t = LocalTransport()
    await t.connect()
    yield t
    await t.disconnect()


@pytest_asyncio.fixture
async def ws_transport():
    if not WS_URL:
        pytest.skip("CUA_TEST_WS_URL not set")
    t = WebSocketTransport(WS_URL, api_key=API_KEY)
    await t.connect()
    yield t
    await t.disconnect()


@pytest_asyncio.fixture
async def http_transport():
    if not HTTP_URL:
        pytest.skip("CUA_TEST_HTTP_URL not set")
    t = HTTPTransport(HTTP_URL, api_key=API_KEY, container_name=CONTAINER_NAME)
    await t.connect()
    yield t
    await t.disconnect()


# ---------------------------------------------------------------------------
# Sandbox fixtures (one per transport)
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def local_sandbox():
    if not LOCAL_ENABLED:
        pytest.skip("CUA_TEST_LOCAL disabled")
    async with Localhost.connect() as host:
        yield host


@pytest_asyncio.fixture
async def ws_sandbox():
    if not WS_URL:
        pytest.skip("CUA_TEST_WS_URL not set")
    sb = await Sandbox._create(ws_url=WS_URL, api_key=API_KEY, name="test-ws")
    yield sb
    await sb.disconnect()


@pytest_asyncio.fixture
async def http_sandbox():
    if not HTTP_URL:
        pytest.skip("CUA_TEST_HTTP_URL not set")
    sb = await Sandbox._create(
        http_url=HTTP_URL,
        api_key=API_KEY,
        container_name=CONTAINER_NAME,
        name="test-http",
    )
    yield sb
    await sb.disconnect()


@pytest_asyncio.fixture
async def localhost_instance():
    if not LOCAL_ENABLED:
        pytest.skip("CUA_TEST_LOCAL disabled")
    async with Localhost.connect() as host:
        yield host


# ---------------------------------------------------------------------------
# Parametrized "any sandbox" fixture — runs test against every available backend
# ---------------------------------------------------------------------------


def _sandbox_params():
    params = []
    if LOCAL_ENABLED:
        params.append("local_sandbox")
    if WS_URL:
        params.append("ws_sandbox")
    if HTTP_URL:
        params.append("http_sandbox")
    if not params:
        params.append("local_sandbox")  # fallback
    return params


@pytest.fixture(params=_sandbox_params())
def any_sandbox_name(request):
    """Returns the fixture name; used by any_sandbox."""
    return request.param


@pytest_asyncio.fixture
async def any_sandbox(any_sandbox_name, request):
    """Yields a Sandbox connected via whichever transport is being parametrized."""
    # Dynamically request the named fixture
    sb = request.getfixturevalue(any_sandbox_name)
    return sb
