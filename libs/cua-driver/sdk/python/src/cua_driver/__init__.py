"""cua_driver — Python SDK for cua-driver UI automation.

Sync quick start::

    from cua_driver import DriverClient, App, expect

    with DriverClient() as client:
        with App.launch("com.apple.calculator", client) as app:
            window = app.main_window()
            window.locator("AXButton", label="2").click()
            window.locator("AXButton", label="Add").click()
            window.locator("AXButton", label="2").click()
            window.locator("AXButton", label="Equals").click()
            expect(window.locator("AXStaticText")).to_contain_text("4")

Async quick start (pytest-asyncio)::

    from cua_driver import DriverClient, AsyncApp, async_expect

    async def test_calc(cua_driver):          # cua_driver fixture from pytest_cua
        async with AsyncApp.launch("com.apple.calculator", cua_driver) as app:
            window = await app.main_window()
            await window.locator("AXButton", label="2").click()
            await window.locator("AXButton", label="Add").click()
            await window.locator("AXButton", label="2").click()
            await window.locator("AXButton", label="Equals").click()
            await async_expect(window.locator("AXStaticText")).to_contain_text("4")
"""

# Sync API
from ._client import DriverClient, MCPCallError, default_binary_path
from ._app import App, kill_app
from ._window import Window
from ._locator import Locator
from ._expect import expect, LocatorAssertions

# Async API
from ._async import (
    AsyncDriverClient,
    AsyncApp,
    AsyncWindow,
    AsyncLocator,
    AsyncLocatorAssertions,
    async_expect,
)

__all__ = [
    # Sync
    "DriverClient",
    "MCPCallError",
    "default_binary_path",
    "App",
    "kill_app",
    "Window",
    "Locator",
    "expect",
    "LocatorAssertions",
    # Async
    "AsyncDriverClient",
    "AsyncApp",
    "AsyncWindow",
    "AsyncLocator",
    "AsyncLocatorAssertions",
    "async_expect",
]
