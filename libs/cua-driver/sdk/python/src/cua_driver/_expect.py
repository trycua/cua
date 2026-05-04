"""expect() — assertion helper for cua-driver Locators.

Modelled after Playwright's ``expect(locator).to_have_value(...)`` API.
Each assertion retries until the condition holds or the timeout elapses.
"""

from __future__ import annotations

import time
from typing import Optional

from ._locator import Locator


class LocatorAssertions:
    """Fluent assertion interface for a :class:`~cua_driver.Locator`.

    Obtain via :func:`expect`::

        expect(window.locator("AXStaticText")).to_contain_text("4")
    """

    def __init__(self, locator: Locator, timeout: float = 5.0) -> None:
        self._locator = locator
        self._timeout = timeout

    # ------------------------------------------------------------------
    # Positive assertions
    # ------------------------------------------------------------------

    def to_be_visible(self, timeout: Optional[float] = None) -> None:
        """Assert the element is present in the AX tree."""
        t = timeout if timeout is not None else self._timeout
        # _wait_for_element raises TimeoutError on failure, which is an
        # assertion error in a test context.
        self._locator._wait_for_element(t)

    def to_contain_text(self, text: str, timeout: Optional[float] = None) -> None:
        """Assert that at least one matching element's visible text contains *text*.

        Checks ALL elements returned by the locator — important when there are
        multiple elements of the same role (e.g. the Calculator result display
        and the expression-history display both being ``AXStaticText``).
        """
        t = timeout if timeout is not None else self._timeout
        deadline = time.monotonic() + t
        last_content = "<not found>"
        while True:
            contents = self._locator.all_text_contents()
            for content in contents:
                if text in content:
                    return
            if contents:
                last_content = repr(contents)
            if time.monotonic() >= deadline:
                raise AssertionError(
                    f"Expected element to contain text {text!r}, "
                    f"but got {last_content} after {t}s"
                )
            time.sleep(0.3)

    def to_have_text(self, text: str, timeout: Optional[float] = None) -> None:
        """Assert that at least one matching element's visible text equals *text* exactly."""
        t = timeout if timeout is not None else self._timeout
        deadline = time.monotonic() + t
        last_content = "<not found>"
        while True:
            contents = self._locator.all_text_contents()
            for content in contents:
                if content == text:
                    return
            if contents:
                last_content = repr(contents)
            if time.monotonic() >= deadline:
                raise AssertionError(
                    f"Expected element text to equal {text!r}, "
                    f"but got {last_content} after {t}s"
                )
            time.sleep(0.3)

    def to_have_value(self, value: str, timeout: Optional[float] = None) -> None:
        """Assert the element's ``value=`` attribute equals *value*."""
        t = timeout if timeout is not None else self._timeout
        deadline = time.monotonic() + t
        last_val = "<not found>"
        while True:
            try:
                val = self._locator.get_value(timeout=0.0)
                if val == value:
                    return
                last_val = val
            except TimeoutError:
                pass
            if time.monotonic() >= deadline:
                raise AssertionError(
                    f"Expected element value to equal {value!r}, "
                    f"but got {last_val!r} after {t}s"
                )
            time.sleep(0.3)

    # ------------------------------------------------------------------
    # Negative assertions
    # ------------------------------------------------------------------

    def not_to_be_visible(self, timeout: Optional[float] = None) -> None:
        """Assert the element is NOT present in the AX tree."""
        t = timeout if timeout is not None else self._timeout
        deadline = time.monotonic() + t
        while True:
            if not self._locator.is_visible(timeout=0.0):
                return
            if time.monotonic() >= deadline:
                raise AssertionError(
                    f"Expected element to not be visible, but it was still "
                    f"present after {t}s"
                )
            time.sleep(0.3)


def expect(locator: Locator, timeout: float = 5.0) -> LocatorAssertions:
    """Return a :class:`LocatorAssertions` for *locator*.

    Usage::

        from cua_driver import expect

        expect(window.locator("AXStaticText")).to_contain_text("4")
        expect(window.locator("AXButton", label="OK")).to_be_visible()
    """
    return LocatorAssertions(locator, timeout)
