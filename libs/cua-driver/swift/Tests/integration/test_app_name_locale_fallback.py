"""Integration test: app name resolution fallback (#1481).

Verifies that launch_app accepts more than just the exact bundle-file name:

  1. Bundle ID passed as `name` (e.g. "com.apple.calculator") resolves via
     LaunchServices — callers don't need to use the `bundle_id` parameter.

  2. Case-insensitive match on CFBundleName / display name works so
     "calculator" and "CALCULATOR" both resolve to Calculator.app.

  3. Exact name still works (regression guard).

The JP-locale localizedName path ("計算機") cannot be exercised on an EN
locale machine; that path is exercised by the same NSRunningApplication
localizedName lookup and would pass on a JP-locale host.

Run:
    scripts/test.sh test_app_name_locale_fallback
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path, reset_calculator

CALCULATOR_BUNDLE = "com.apple.calculator"


def _kill_calc() -> None:
    subprocess.run(["pkill", "-x", "Calculator"], check=False)
    time.sleep(0.4)


class AppNameLocaleFallbackTests(unittest.TestCase):
    """launch_app name= accepts bundle IDs and case-insensitive display names."""

    def setUp(self) -> None:
        reset_calculator()
        self.client = DriverClient(default_binary_path()).__enter__()

    def tearDown(self) -> None:
        self.client.__exit__(None, None, None)
        _kill_calc()

    def _launch_by_name(self, name: str) -> dict:
        result = self.client.call_tool("launch_app", {"name": name})
        return result

    def test_bundle_id_as_name_resolves(self) -> None:
        """Bundle ID string passed as name= launches the correct app."""
        result = self._launch_by_name(CALCULATOR_BUNDLE)
        self.assertFalse(
            result.get("isError"),
            f"launch_app(name='{CALCULATOR_BUNDLE}') failed: {result}",
        )
        sc = result.get("structuredContent", {})
        self.assertEqual(
            sc.get("bundle_id"),
            CALCULATOR_BUNDLE,
            f"Unexpected bundle_id in response: {sc}",
        )
        self.assertGreater(sc.get("pid", 0), 0)

    def test_case_insensitive_name_lowercase(self) -> None:
        """Lowercase name= ('calculator') matches Calculator.app."""
        result = self._launch_by_name("calculator")
        self.assertFalse(
            result.get("isError"),
            f"launch_app(name='calculator') failed: {result}",
        )
        sc = result.get("structuredContent", {})
        self.assertEqual(sc.get("bundle_id"), CALCULATOR_BUNDLE)

    def test_case_insensitive_name_uppercase(self) -> None:
        """All-caps name= ('CALCULATOR') matches Calculator.app."""
        result = self._launch_by_name("CALCULATOR")
        self.assertFalse(
            result.get("isError"),
            f"launch_app(name='CALCULATOR') failed: {result}",
        )
        sc = result.get("structuredContent", {})
        self.assertEqual(sc.get("bundle_id"), CALCULATOR_BUNDLE)

    def test_exact_name_still_works(self) -> None:
        """Exact canonical name= ('Calculator') still works (regression guard)."""
        result = self._launch_by_name("Calculator")
        self.assertFalse(
            result.get("isError"),
            f"launch_app(name='Calculator') failed: {result}",
        )
        sc = result.get("structuredContent", {})
        self.assertEqual(sc.get("bundle_id"), CALCULATOR_BUNDLE)

    def test_unknown_name_returns_error(self) -> None:
        """Completely unknown name returns isError (not a silent empty result)."""
        result = self._launch_by_name("ThisAppDefinitelyDoesNotExist_xyzzy")
        self.assertTrue(
            result.get("isError"),
            f"Expected isError for unknown app name, got: {result}",
        )


if __name__ == "__main__":
    unittest.main()
