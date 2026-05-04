"""pytest-cua: pytest fixtures for cua-driver UI tests.

Fixtures provided
-----------------
Sync (work with or without pytest-asyncio):
  cua_binary       — path to the cua-driver binary (session scope, overridable)
  cua_driver       — DriverClient per test function (new process each test)
  cua_session      — DriverClient shared across the session (faster, less isolated)
  cua_app          — launches + quits an app by bundle_id (indirect parametrize)

Async (require pytest-asyncio):
  async_cua_driver — DriverClient per test (async-compatible, still sync under the hood)
  async_cua_app    — async AsyncApp per test (indirect parametrize)

Usage examples
--------------
Sync::

    @pytest.mark.parametrize("cua_app", ["com.apple.calculator"], indirect=True)
    def test_addition(cua_app):
        window = cua_app.main_window()
        window.locator("AXButton", label="2").click()
        ...

Async (pytest-asyncio, asyncio_mode="auto")::

    @pytest.mark.parametrize("async_cua_app", ["com.apple.calculator"], indirect=True)
    async def test_addition_async(async_cua_app):
        window = await async_cua_app.main_window()
        await window.locator("AXButton", label="2").click()
        ...

Parallel (pytest-xdist, --dist loadfile)::

    # Each .py file runs in its own xdist worker → separate cua-driver process.
    # Module-scoped cua_driver fixture is local to each worker.
    # Add to pytest.ini:  addopts = -n auto --dist loadfile
"""

from __future__ import annotations

import pytest

from cua_driver._client import DriverClient, default_binary_path
from cua_driver._app import App
from cua_driver._async import AsyncApp


# ---------------------------------------------------------------------------
# Sync fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def cua_binary() -> str:
    """Path to the cua-driver binary.

    Override by setting the ``CUA_DRIVER_BINARY`` environment variable or
    by defining your own ``cua_binary`` fixture in ``conftest.py``.
    """
    return default_binary_path()


@pytest.fixture
def cua_driver(cua_binary: str) -> "DriverClient":
    """Function-scoped :class:`~cua_driver.DriverClient`.

    A fresh cua-driver process is spawned for each test and terminated
    afterwards.  Safest isolation; use ``cua_session`` for speed.
    """
    with DriverClient(cua_binary) as client:
        yield client


@pytest.fixture(scope="session")
def cua_session(cua_binary: str) -> "DriverClient":
    """Session-scoped :class:`~cua_driver.DriverClient`.

    One cua-driver process shared across the entire test session.
    Faster than ``cua_driver`` but shares AX state across tests.

    With pytest-xdist each worker gets its own copy automatically (xdist
    workers are separate processes with isolated session scopes).
    """
    with DriverClient(cua_binary) as client:
        yield client


@pytest.fixture
def cua_app(request: pytest.FixtureRequest, cua_driver: DriverClient) -> "App":
    """Launch an app by bundle_id and quit it after the test.

    Use with ``indirect`` parametrization::

        @pytest.mark.parametrize("cua_app", ["com.apple.calculator"], indirect=True)
        def test_calc(cua_app):
            ...
    """
    bundle_id: str = request.param
    app = App.launch(bundle_id, cua_driver)
    yield app
    app.quit()


# ---------------------------------------------------------------------------
# Async fixtures — only available when pytest-asyncio is installed
# ---------------------------------------------------------------------------

try:
    import pytest_asyncio as _pytest_asyncio  # noqa: F401
    _has_asyncio = True
except ImportError:
    _has_asyncio = False


if _has_asyncio:
    import pytest_asyncio

    @pytest_asyncio.fixture
    async def async_cua_driver(cua_binary: str) -> "DriverClient":
        """Async-compatible function-scoped DriverClient.

        The underlying client is sync (safe to call from ``asyncio.to_thread``
        inside async tests).  Requires pytest-asyncio.
        """
        with DriverClient(cua_binary) as client:
            yield client

    @pytest_asyncio.fixture
    async def async_cua_app(
        request: pytest.FixtureRequest,
        async_cua_driver: DriverClient,
    ) -> "AsyncApp":
        """Launch an app asynchronously and quit it after the test.

        Use with ``indirect`` parametrization::

            @pytest.mark.parametrize("async_cua_app", ["com.apple.calculator"], indirect=True)
            async def test_calc(async_cua_app):
                window = await async_cua_app.main_window()
                ...

        Requires pytest-asyncio.
        """
        bundle_id: str = request.param
        app = await AsyncApp.launch(bundle_id, async_cua_driver)
        yield app
        await app.quit()

else:
    # Provide stub fixtures that raise a clear error when pytest-asyncio is absent
    @pytest.fixture
    def async_cua_driver():
        pytest.skip("async_cua_driver requires pytest-asyncio (pip install pytest-asyncio)")

    @pytest.fixture
    def async_cua_app():
        pytest.skip("async_cua_app requires pytest-asyncio (pip install pytest-asyncio)")
