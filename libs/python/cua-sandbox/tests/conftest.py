"""Shared fixtures for cua-sandbox integration tests.

Each transport/runtime is exposed as a pytest fixture. Tests that need a
specific backend request the fixture by name; parametrized tests pull from
all available backends.

Environment variables control which backends are exercised:

    CUA_TEST_LOCAL=1              Force localhost/local-sandbox tests on/off.
                                  Unset means "auto": they run only when this
                                  host actually has a controllable desktop
                                  (pynput importable + a screenshot succeeds),
                                  so a headless CI runner skips them instead of
                                  failing on a missing DISPLAY.
    CUA_TEST_WS_URL=ws://...     Enable WebSocket transport tests
    CUA_TEST_HTTP_URL=http://...  Enable HTTP transport tests
    CUA_TEST_API_KEY=sk-...       API key for remote transports
    CUA_TEST_CONTAINER_NAME=...   Container name for HTTP cloud auth
"""

from __future__ import annotations

import functools
import os
import subprocess
import sys

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


_DESKTOP_PROBE = """
import cua_auto.keyboard
import cua_auto.mouse
from cua_auto.screen import screenshot

screenshot()
"""


@functools.lru_cache(maxsize=1)
def local_desktop_available() -> bool:
    """True when this host can actually be driven as a Localhost sandbox.

    The Localhost backend types on the real keyboard and grabs the real screen
    through cua-auto/pynput. On a headless machine (CI runners included) the
    import raises, so every localhost test fails for a reason that has nothing
    to do with the code under test.

    The probe runs in a subprocess on purpose: against an X display with no
    window manager, pynput's Xlib backend terminates the interpreter outright
    rather than raising, which would take the whole pytest session down during
    collection.
    """
    if sys.platform.startswith("linux") and not (
        os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")
    ):
        return False
    try:
        proc = subprocess.run(
            [sys.executable, "-c", _DESKTOP_PROBE],
            capture_output=True,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return proc.returncode == 0


def _local_enabled() -> bool:
    if os.environ.get("CUA_TEST_LOCAL", ""):
        return _env_bool("CUA_TEST_LOCAL")
    return local_desktop_available()


LOCAL_ENABLED = _local_enabled()
LOCAL_SKIP_REASON = "no controllable local desktop (set CUA_TEST_LOCAL=1 to force, 0 to silence)"

# Tests that provision a real sandbox — pull a multi-gigabyte image, boot a VM or
# an emulator, then wait for it to answer — are opt-in. Whether the tooling is
# installed is not the question: a GitHub runner ships the Android SDK and Docker
# and still cannot boot either in the time a PR check is allowed to take.
RUNTIME_TESTS_ENABLED = _env_bool("CUA_TEST_RUNTIME")
RUNTIME_SKIP_REASON = (
    "provisions a real sandbox (multi-GB pull + boot); set CUA_TEST_RUNTIME=1 to run"
)
requires_runtime_optin = pytest.mark.skipif(not RUNTIME_TESTS_ENABLED, reason=RUNTIME_SKIP_REASON)
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
        pytest.skip(LOCAL_SKIP_REASON)
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
        pytest.skip(LOCAL_SKIP_REASON)
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


@pytest.fixture
def any_sandbox(any_sandbox_name, request):
    """Returns a Sandbox connected via whichever transport is being parametrized.

    Deliberately a *sync* fixture: the backend fixtures it forwards to are
    async-generator fixtures, and pytest-asyncio can only set those up from
    outside a running event loop. Resolving them from an async fixture raises
    "Runner.run() cannot be called from a running event loop".
    """
    return request.getfixturevalue(any_sandbox_name)
