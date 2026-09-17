from __future__ import annotations

import argparse
import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run import Driver, optional_visual_observation, select_tab_id, validate_fixture_url


FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"


class FakeSession:
    def __init__(self, structured=None) -> None:
        self.calls = []
        self.structured = structured or {"status": "ok"}

    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        return SimpleNamespace(isError=False, structuredContent=self.structured)


class DriverAdapterTest(unittest.IsolatedAsyncioTestCase):
    async def test_driver_repeats_explicit_session_label(self) -> None:
        session = FakeSession()
        driver = Driver(session, "jev-test")
        await driver.call("browser_type", {"ref": "p2:0"})
        self.assertEqual(
            session.calls,
            [("browser_type", {"ref": "p2:0", "session": "jev-test"})],
        )

    async def test_visual_tool_is_optional_and_uses_the_released_contract_when_advertised(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        session = FakeSession(payload)
        driver = Driver(session, "jev-test")
        snapshot = {
            "target_id": "target",
            "tab_id": "tab",
            "capture_id": "capture-submit",
        }
        self.assertIsNone(await optional_visual_observation(driver, snapshot, set()))
        visual = await optional_visual_observation(driver, snapshot, {"parse_visual_regions"})
        self.assertEqual(visual.capture_id, "capture-submit")
        self.assertEqual(
            session.calls,
            [
                (
                    "parse_visual_regions",
                    {
                        "capture_id": "capture-submit",
                        "options": {
                            "kinds": ["text", "icon"],
                            "min_confidence": 0.8,
                            "max_regions": 100,
                        },
                        "session": "jev-test",
                    },
                )
            ],
        )

    def test_fixture_url_is_confined_to_loopback_http(self) -> None:
        self.assertEqual(
            validate_fixture_url("http://127.0.0.1:8765"), "http://127.0.0.1:8765/"
        )
        for value in ("https://127.0.0.1/", "http://example.com/", "http://localhost/api/"):
            with self.assertRaises(argparse.ArgumentTypeError):
                validate_fixture_url(value)

    def test_tab_selection_accepts_unknown_active_state(self) -> None:
        self.assertEqual(
            select_tab_id([{"tab_id": "first", "active": None}]),
            "first",
        )
        self.assertEqual(
            select_tab_id(
                [
                    {"tab_id": "first", "active": False},
                    {"tab_id": "second", "active": True},
                ]
            ),
            "second",
        )
        with self.assertRaises(RuntimeError):
            select_tab_id([])


if __name__ == "__main__":
    unittest.main()
