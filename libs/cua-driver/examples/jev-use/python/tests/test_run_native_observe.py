"""observe() asks for the full get_window_state response, on any Driver."""

from __future__ import annotations

import asyncio
import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run import DriverToolError
from run_native import observe

FIXTURE = Path(__file__).resolve().parents[2] / "fixtures/native/appkit-window-state-initial-v1.json"


class FakeDriver:
    def __init__(self, refuse_full_output: bool) -> None:
        self.refuse_full_output = refuse_full_output
        self.calls: list[dict] = []

    async def call(self, name, arguments):
        self.calls.append(dict(arguments))
        if self.refuse_full_output and "full_output" in arguments:
            raise DriverToolError(
                "get_window_state failed: [TextContent(type='text', "
                "text='get_window_state: unknown argument full_output')]"
            )
        return json.loads(FIXTURE.read_text())


TASK = SimpleNamespace(scope=SimpleNamespace(window_state_arguments=lambda: {}))


class ObserveTests(unittest.TestCase):
    def setUp(self) -> None:
        payload = json.loads(FIXTURE.read_text())
        self.pid, self.window_id = payload["pid"], payload["window_id"]

    def test_asks_for_the_full_response(self) -> None:
        driver = FakeDriver(refuse_full_output=False)
        asyncio.run(observe(driver, TASK, self.pid, self.window_id))
        self.assertEqual([call.get("full_output") for call in driver.calls], [True])

    def test_retries_without_the_flag_on_drivers_before_0_35(self) -> None:
        driver = FakeDriver(refuse_full_output=True)
        observation = asyncio.run(observe(driver, TASK, self.pid, self.window_id))
        self.assertTrue(observation.elements)
        self.assertEqual(["full_output" in call for call in driver.calls], [True, False])


if __name__ == "__main__":
    unittest.main()
