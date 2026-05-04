"""Calculator addition tests — sync, runs in its own xdist worker.

With ``pytest -n auto --dist loadfile``, this file runs in an isolated
worker process alongside test_calc_multiplication.py.  Each file launches
its own Calculator instance and its own cua-driver subprocess, so the two
suites run concurrently without interfering.

Run a single file (serial):
    pytest sdk/examples/python/test_calc_addition.py -v

Run in parallel with multiplication tests:
    pytest sdk/examples/python/ -n auto --dist loadfile -v
"""

from __future__ import annotations

import subprocess
import time

import pytest

from cua_driver import App, DriverClient, expect, kill_app


# ---------------------------------------------------------------------------
# Module-scoped fixtures — created ONCE per xdist worker (= once per file)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def driver():
    """One cua-driver process for the whole file."""
    with DriverClient() as client:
        yield client


@pytest.fixture(scope="module")
def calc(driver: DriverClient):
    """One Calculator instance for all addition tests in this file."""
    kill_app("com.apple.calculator")
    time.sleep(0.4)
    with App.launch("com.apple.calculator", driver, settle=1.5) as app:
        yield app


@pytest.fixture(autouse=True)
def clear_calc(calc: App):
    """Clear Calculator to 0 before each test."""
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

def test_2_plus_2(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="2").click()
    window.locator("AXButton", label="Add").click()
    window.locator("AXButton", label="2").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("4")


def test_1_plus_1(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="1").click()
    window.locator("AXButton", label="Add").click()
    window.locator("AXButton", label="1").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("2")


def test_3_plus_4(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="3").click()
    window.locator("AXButton", label="Add").click()
    window.locator("AXButton", label="4").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("7")


def test_9_plus_9(calc: App) -> None:
    window = calc.main_window()
    window.locator("AXButton", label="9").click()
    window.locator("AXButton", label="Add").click()
    window.locator("AXButton", label="9").click()
    window.locator("AXButton", label="Equals").click()
    expect(window.locator("AXStaticText")).to_contain_text("18")
