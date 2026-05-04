"""Calculator multiplication tests — sync, runs in its own xdist worker.

Runs CONCURRENTLY with test_calc_addition.py (separate Calculator window,
separate cua-driver process, separate xdist worker).

Run in parallel:
    pytest sdk/examples/python/ -n auto --dist loadfile -v
"""

from __future__ import annotations

import subprocess
import time

import pytest

from cua_driver import App, DriverClient, expect, kill_app


@pytest.fixture(scope="module")
def driver():
    with DriverClient() as client:
        yield client


@pytest.fixture(scope="module")
def calc(driver: DriverClient):
    kill_app("com.apple.calculator")
    time.sleep(0.4)
    with App.launch("com.apple.calculator", driver, settle=1.5) as app:
        yield app


@pytest.fixture(autouse=True)
def clear_calc(calc: App):
    window = calc.main_window()
    try:
        window.locator("AXButton", label="All Clear").click(timeout=2.0)
    except TimeoutError:
        try:
            window.locator("AXButton", label="Clear").click(timeout=2.0)
        except TimeoutError:
            pass
    time.sleep(0.2)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_2_times_3(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="2").click()
    window.locator("AXButton", label="Multiply").click()
    window.locator("AXButton", label="3").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("6")


def test_4_times_4(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="4").click()
    window.locator("AXButton", label="Multiply").click()
    window.locator("AXButton", label="4").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("16")


def test_5_times_3(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="5").click()
    window.locator("AXButton", label="Multiply").click()
    window.locator("AXButton", label="3").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("15")


def test_7_times_8(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="7").click()
    window.locator("AXButton", label="Multiply").click()
    window.locator("AXButton", label="8").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("56")
