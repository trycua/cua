"""Calculator tests using the async API (pytest-asyncio + xdist).

Demonstrates:
  - ``async def test_...`` functions with pytest-asyncio in auto mode
  - ``AsyncApp`` / ``AsyncLocator`` / ``async_expect()``
  - Module-scoped async fixtures shared within the file
  - Parallel execution: this file runs in its own xdist worker alongside
    the two sync calculator files → THREE Calculator instances concurrently

Requirements:
    pip install pytest-asyncio pytest-xdist

Run just this file:
    pytest sdk/examples/python/test_calc_async.py -v --asyncio-mode=auto

Run all three calculator files in parallel:
    pytest sdk/examples/python/test_calc_*.py -n auto --dist loadfile --asyncio-mode=auto
"""

from __future__ import annotations

import asyncio
import time

import pytest

from cua_driver import AsyncApp, DriverClient, async_expect, kill_app


# ---------------------------------------------------------------------------
# Module-scoped fixtures (async, run once per xdist worker = once per file)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def driver():
    """Sync DriverClient — used inside AsyncApp via asyncio.to_thread."""
    with DriverClient() as client:
        yield client


@pytest.fixture(scope="module")
async def calc(driver: DriverClient):
    """One AsyncApp for all async tests in this file."""
    kill_app("com.apple.calculator")
    await asyncio.sleep(0.4)
    app = await AsyncApp.launch("com.apple.calculator", driver, settle=1.5)
    try:
        yield app
    finally:
        await app.quit()


@pytest.fixture(autouse=True)
async def clear_calc(calc: AsyncApp):
    """Clear calculator state between tests."""
    window = await calc.main_window()
    try:
        await window.locator("AXButton", label="All Clear").click(timeout=2.0)
    except TimeoutError:
        try:
            await window.locator("AXButton", label="Clear").click(timeout=2.0)
        except TimeoutError:
            pass


# ---------------------------------------------------------------------------
# Async tests
# ---------------------------------------------------------------------------

async def test_async_2_plus_3(calc: AsyncApp) -> None:
    window = await calc.main_window()
    await window.locator("AXButton", label="2").click()
    await window.locator("AXButton", label="Add").click()
    await window.locator("AXButton", label="3").click()
    await window.locator("AXButton", label="Equals").click()
    await async_expect(window.locator("AXStaticText")).to_contain_text("5")


async def test_async_8_minus_3(calc: AsyncApp) -> None:
    window = await calc.main_window()
    await window.locator("AXButton", label="8").click()
    await window.locator("AXButton", label="Subtract").click()
    await window.locator("AXButton", label="3").click()
    await window.locator("AXButton", label="Equals").click()
    await async_expect(window.locator("AXStaticText")).to_contain_text("5")


async def test_async_6_divided_by_2(calc: AsyncApp) -> None:
    window = await calc.main_window()
    await window.locator("AXButton", label="6").click()
    await window.locator("AXButton", label="Divide").click()
    await window.locator("AXButton", label="2").click()
    await window.locator("AXButton", label="Equals").click()
    await async_expect(window.locator("AXStaticText")).to_contain_text("3")


async def test_5_plus_5(calc: AsyncApp) -> None:
    """5 + 5 = 10 — uses the module-scoped async calc fixture."""
    window = await calc.main_window()
    await window.locator("AXButton", label="5").click()
    await window.locator("AXButton", label="Add").click()
    await window.locator("AXButton", label="5").click()
    await window.locator("AXButton", label="Equals").click()
    await async_expect(window.locator("AXStaticText")).to_contain_text("10")
