from __future__ import annotations

import argparse
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run import Driver, validate_fixture_url


class FakeSession:
    def __init__(self) -> None:
        self.calls = []

    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        return SimpleNamespace(isError=False, structuredContent={"status": "ok"})


class DriverAdapterTest(unittest.IsolatedAsyncioTestCase):
    async def test_driver_repeats_explicit_session_label(self) -> None:
        session = FakeSession()
        driver = Driver(session, "jev-test")
        await driver.call("browser_type", {"ref": "p2:0"})
        self.assertEqual(
            session.calls,
            [("browser_type", {"ref": "p2:0", "session": "jev-test"})],
        )

    def test_fixture_url_is_confined_to_loopback_http(self) -> None:
        self.assertEqual(
            validate_fixture_url("http://127.0.0.1:8765"), "http://127.0.0.1:8765/"
        )
        for value in ("https://127.0.0.1/", "http://example.com/", "http://localhost/api/"):
            with self.assertRaises(argparse.ArgumentTypeError):
                validate_fixture_url(value)


if __name__ == "__main__":
    unittest.main()
