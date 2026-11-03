"""Shared fixtures for cua-sandbox integration tests.

Each transport/runtime is exposed as a pytest fixture. Tests that need a
specific backend request the fixture by name; parametrized tests pull from
all available backends.

Environment variables control which backends are exercised:

    CUA_TEST_ENV_URL=http://h:3211    Enable tests against a running cua-spacesd
    CUA_TEST_ENV_TOKEN=...            Its token, when it requires one

Without ``CUA_TEST_ENV_URL`` the integration tests skip. Nothing here drives
the host machine: Localhost was removed (use cua-driver for local control).
"""

from __future__ import annotations

import os
from typing import Any

import pytest
import pytest_asyncio
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.env import EnvTransport


@pytest.fixture(autouse=True)
def _offline_image_resolver(request, monkeypatch):
    """The one image resolver (native cua_image) never reads a registry in
    unit tests; tests that need an answer patch ``native().resolve_image``.
    Live suites keep the real resolver."""
    if not str(request.node.path).startswith(_LIVE_DIR):
        monkeypatch.setenv("CUA_IMAGE_RESOLVE", "0")
    yield


class _NoRealLocalSandboxes:
    """``local_runtime()`` in unit tests: creating a real local sandbox
    (container, QEMU, Lume) fails loudly instead of touching this machine.
    Tests that need a local runtime patch ``local_runtime`` themselves."""

    def __init__(self, real: Any) -> None:
        self._real = real

    def sandboxes(self) -> Any:
        real = self._real().sandboxes()

        class _Guard:
            def __getattr__(self, name: str) -> Any:
                if name == "create":
                    raise AssertionError(
                        "a unit test tried to start a real local sandbox; patch local_runtime"
                    )
                return getattr(real, name)

        return _Guard()

    def __getattr__(self, name: str) -> Any:
        return getattr(self._real(), name)


@pytest.fixture(autouse=True)
def _no_real_local_sandboxes(request, monkeypatch):
    if str(request.node.path).startswith(_LIVE_DIR):
        yield
        return
    import cua_sandbox._sdk as sdk_module
    import cua_sandbox.runtime.native as native_module

    real = sdk_module.local_runtime
    guard = _NoRealLocalSandboxes(real)
    monkeypatch.setattr(sdk_module, "local_runtime", lambda: guard)
    monkeypatch.setattr(native_module, "local_runtime", lambda: guard)
    yield


# ---------------------------------------------------------------------------
# Hermetic env: keep live-gate and credential env out of unit tests
# ---------------------------------------------------------------------------

# Variables a developer shell (or CI's live lanes) may export that change how
# unit tests behave: live-E2E knobs, live opt-in gates, API-key credentials and
# the built-in image overrides.
_LIVE_ENV_PREFIXES = ("CUA_LIVE_E2E_", "CUA_TEST_LOCAL_")
_LIVE_ENV_NAMES = frozenset(
    {
        "CUA_TEST_ANDROID_MULTITOUCH",
        "CUA_API_KEY",
        "CUA_BASE_URL",
        "CUA_DEFAULT_LINUX_IMAGE",
        "CUA_DEFAULT_WINDOWS_IMAGE",
        "CUA_SANDBOX_LINUX_CONTAINER_IMAGE",
        "CUA_IMAGE_LINUX",
        "CUA_IMAGE_WINDOWS",
        "CUA_IMAGE_MACOS",
        # User defaults (`cua config`): where, what kind, which engine.
        "CUA_DEFAULT_ON",
        "CUA_DEFAULT_KIND",
        "CUA_DEFAULT_RUNTIME",
    }
)
_LIVE_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "live") + os.sep


def _is_live_env_name(name: str) -> bool:
    return name in _LIVE_ENV_NAMES or name.startswith(_LIVE_ENV_PREFIXES)


@pytest.fixture(autouse=True)
def _isolate_live_env(request, monkeypatch, tmp_path_factory):
    """Unset live-gate env for every test outside ``tests/live``.

    Unit tests that need one of these variables set it with ``monkeypatch``;
    anything a test sets is undone afterwards, so nothing leaks between tests
    or in from the developer's shell. Live suites under ``tests/live`` opt in
    explicitly and keep the real environment.
    """
    if str(request.node.path).startswith(_LIVE_DIR):
        return
    for name in [n for n in os.environ if _is_live_env_name(n)]:
        monkeypatch.delenv(name, raising=False)
    # Never read or write the real ~/.cua: sandbox records, credentials and
    # caches all live under CUA_HOME (cua_sandbox._paths), here a temp dir.
    monkeypatch.setenv("CUA_HOME", str(tmp_path_factory.mktemp("cua-home")))
    # Never read the user's `cua auth login` session (the OS keychain) in
    # unit tests.
    monkeypatch.setenv("CUA_CREDENTIAL_STORE", "file")


# ---------------------------------------------------------------------------
# Helper: read env config
# ---------------------------------------------------------------------------


ENV_URL = os.environ.get("CUA_TEST_ENV_URL")
ENV_TOKEN = os.environ.get("CUA_TEST_ENV_TOKEN")


# ---------------------------------------------------------------------------
# Transport fixtures
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def env_transport():
    if not ENV_URL:
        pytest.skip("CUA_TEST_ENV_URL not set")
    t = EnvTransport(url=ENV_URL, token=ENV_TOKEN)
    await t.connect()
    yield t
    await t.disconnect()


# ---------------------------------------------------------------------------
# Sandbox fixtures (one per transport)
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def env_sandbox():
    if not ENV_URL:
        pytest.skip("CUA_TEST_ENV_URL not set")
    sb = await Sandbox.connect(url=ENV_URL, token=ENV_TOKEN)
    yield sb
    await sb.disconnect()


# ---------------------------------------------------------------------------
# Parametrized "any sandbox" fixture — runs test against every available backend
# ---------------------------------------------------------------------------


def _sandbox_params():
    # env_sandbox skips itself when CUA_TEST_ENV_URL is unset.
    return ["env_sandbox"]


@pytest.fixture(params=_sandbox_params())
def any_sandbox_name(request):
    """Returns the fixture name; used by any_sandbox."""
    return request.param


@pytest.fixture
def any_sandbox(any_sandbox_name, request):
    """Yields a Sandbox connected via whichever transport is being parametrized."""
    # Dynamically request the named fixture. A sync fixture: pytest-asyncio
    # (>=1.0) cannot resolve an async fixture from inside another one.
    return request.getfixturevalue(any_sandbox_name)
