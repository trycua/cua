"""Example: Calculator arithmetic using cua_driver SDK.

Demonstrates the Playwright-style high-level API:
  - App.launch() / context manager
  - window.locator(role, label=...).click()
  - expect(locator).to_contain_text()

Run::

    cd libs/cua-driver
    python -m pytest sdk/examples/python/test_calculator.py -v
"""

from __future__ import annotations

import subprocess
import time

import pytest

from cua_driver import App, DriverClient, expect


# ---------------------------------------------------------------------------
# Fixture: session-scoped DriverClient + Calculator cleanup
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def driver():
    with DriverClient() as client:
        yield client


@pytest.fixture(autouse=True)
def kill_calculator():
    """Kill Calculator before and after each test for a clean state."""
    subprocess.run(["pkill", "-x", "Calculator"], check=False)
    time.sleep(0.3)
    yield
    subprocess.run(["pkill", "-x", "Calculator"], check=False)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_2_plus_2(driver: DriverClient) -> None:
    """2 + 2 = 4 via AX element clicks."""
    with App.launch("com.apple.calculator", driver, settle=1.5) as app:
        window = app.main_window()

        window.locator("AXButton", label="2").click()
        window.locator("AXButton", label="Add").click()
        window.locator("AXButton", label="2").click()
        window.locator("AXButton", label="Equals").click()

        expect(window.locator("AXStaticText")).to_contain_text("4")


def test_3_plus_4(driver: DriverClient) -> None:
    """3 + 4 = 7 via AX element clicks."""
    with App.launch("com.apple.calculator", driver, settle=1.5) as app:
        window = app.main_window()

        window.locator("AXButton", label="3").click()
        window.locator("AXButton", label="Add").click()
        window.locator("AXButton", label="4").click()
        window.locator("AXButton", label="Equals").click()

        expect(window.locator("AXStaticText")).to_contain_text("7")


def test_all_clear(driver: DriverClient) -> None:
    """Press All Clear / Clear and verify display resets."""
    with App.launch("com.apple.calculator", driver, settle=1.5) as app:
        window = app.main_window()

        # Enter something
        window.locator("AXButton", label="5").click()

        # Clear — button may be "All Clear" (AC) or "Clear" (C)
        try:
            window.locator("AXButton", label="All Clear").click()
        except TimeoutError:
            window.locator("AXButton", label="Clear").click()

        expect(window.locator("AXStaticText")).to_contain_text("0")


@pytest.mark.parametrize("cua_app", ["com.apple.calculator"], indirect=True)
def test_5_times_3_via_fixture(cua_app: App) -> None:
    """5 × 3 = 15, using the cua_app pytest fixture from pytest_cua."""
    window = cua_app.main_window()

    window.locator("AXButton", label="5").click()
    window.locator("AXButton", label="Multiply").click()
    window.locator("AXButton", label="3").click()
    window.locator("AXButton", label="Equals").click()

    expect(window.locator("AXStaticText")).to_contain_text("15")
