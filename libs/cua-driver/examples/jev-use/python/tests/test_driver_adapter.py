from __future__ import annotations

import argparse
import asyncio
import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from core import build_candidates, validate_choice
from run import (
    Driver,
    DriverToolError,
    observe_visual,
    optional_visual_observation,
    select_tab_id,
    supports_capture_bound_click,
    validate_fixture_url,
)


FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"


class FakeSession:
    def __init__(self, structured=None, responses=None) -> None:
        self.calls = []
        self.structured = structured or {"status": "ok"}
        self.responses = list(responses or [])

    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        structured = self.responses.pop(0) if self.responses else self.structured
        return SimpleNamespace(isError=False, structuredContent=structured)


class DriverAdapterTest(unittest.IsolatedAsyncioTestCase):
    async def test_driver_repeats_explicit_session_label(self) -> None:
        session = FakeSession()
        driver = Driver(session, "jev-test")
        await driver.call("browser_type", {"ref": "p2:0"})
        self.assertEqual(
            session.calls,
            [("browser_type", {"ref": "p2:0", "session": "jev-test"})],
        )

    async def test_visual_tool_is_optional_and_uses_the_public_contract_when_advertised(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())
        session = FakeSession(responses=[{"capture_id": "capture-submit"}, payload])
        driver = Driver(session, "jev-test")
        self.assertIsNone(
            await optional_visual_observation(
                driver,
                7,
                9,
                {"get_window_state", "parse_visual_regions", "click"},
                False,
            )
        )
        visual = await optional_visual_observation(
            driver,
            7,
            9,
            {"get_window_state", "parse_visual_regions", "click"},
            True,
        )
        self.assertEqual(visual.capture_id, "capture-submit")
        self.assertEqual(
            session.calls,
            [
                (
                    "get_window_state",
                    {
                        "pid": 7,
                        "window_id": 9,
                        "include_accessibility_tree": False,
                        "session": "jev-test",
                    },
                ),
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

    async def test_visual_status_is_logged_for_ok_not_installed_and_error(self) -> None:
        tools = {"get_window_state", "parse_visual_regions", "click"}
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())

        class ScriptedSession(FakeSession):
            def __init__(self, parse_result) -> None:
                super().__init__()
                self.parse_result = parse_result

            async def call_tool(self, name, arguments):
                self.calls.append((name, arguments))
                if name == "get_window_state":
                    return SimpleNamespace(
                        isError=False, structuredContent={"capture_id": "capture-submit"}
                    )
                return self.parse_result

        ok = SimpleNamespace(isError=False, structuredContent=payload)
        visual, status = await observe_visual(
            Driver(ScriptedSession(ok), "jev-test"), 7, 9, tools, True
        )
        self.assertEqual(visual.capture_id, "capture-submit")
        self.assertEqual(
            status,
            {"status": "ok", "capture_id": "capture-submit", "region_count": len(visual.regions)},
        )

        not_installed = SimpleNamespace(
            isError=True,
            structuredContent={"code": "not_installed", "message": "extension missing"},
            content=[{"text": "not installed"}],
        )
        visual, status = await observe_visual(
            Driver(ScriptedSession(not_installed), "jev-test"), 7, 9, tools, True
        )
        self.assertIsNone(visual)
        self.assertEqual(status, {"status": "not_installed", "error_code": "not_installed"})

        worker_failed = SimpleNamespace(
            isError=True,
            structuredContent={"code": "worker_failed"},
            content=[{"text": "worker crashed"}],
        )
        visual, status = await observe_visual(
            Driver(ScriptedSession(worker_failed), "jev-test"), 7, 9, tools, True
        )
        self.assertIsNone(visual)
        self.assertEqual(status, {"status": "error", "error_code": "worker_failed"})

        stale = json.loads(json.dumps(payload))
        stale["capture"]["capture_id"] = "older-capture"
        visual, status = await observe_visual(
            Driver(
                ScriptedSession(SimpleNamespace(isError=False, structuredContent=stale)),
                "jev-test",
            ),
            7,
            9,
            tools,
            True,
        )
        self.assertIsNone(visual)
        self.assertEqual(status, {"status": "error", "error_code": "capture_mismatch"})

        uncoded = SimpleNamespace(isError=True, structuredContent=None, content=[])
        _, status = await observe_visual(
            Driver(ScriptedSession(uncoded), "jev-test"), 7, 9, tools, True
        )
        self.assertEqual(status, {"status": "error", "error_code": "driver_error"})

        session = FakeSession()
        _, status = await observe_visual(Driver(session, "jev-test"), 7, 9, {"click"}, True)
        self.assertEqual(status, {"status": "unavailable", "error_code": "tool_not_advertised"})
        _, status = await observe_visual(Driver(session, "jev-test"), 7, 9, tools, False)
        self.assertEqual(
            status,
            {"status": "unavailable", "error_code": "capture_bound_click_unsupported"},
        )
        self.assertEqual(session.calls, [])

    async def test_driver_error_carries_the_structured_error_code(self) -> None:
        class FailingSession(FakeSession):
            async def call_tool(self, name, arguments):
                return SimpleNamespace(
                    isError=True, structuredContent={"code": "not_installed"}, content=[]
                )

        with self.assertRaises(DriverToolError) as raised:
            await Driver(FailingSession(), "jev-test").call("parse_visual_regions", {})
        self.assertEqual(raised.exception.code, "not_installed")

    def test_capture_bound_click_requires_advertised_capture_id_schema(self) -> None:
        self.assertFalse(
            supports_capture_bound_click(
                [SimpleNamespace(name="click", inputSchema={"properties": {"x": {}}})]
            )
        )
        self.assertTrue(
            supports_capture_bound_click(
                [
                    SimpleNamespace(
                        name="click", inputSchema={"properties": {"capture_id": {}}}
                    )
                ]
            )
        )

    async def test_provider_delay_then_capture_change_refuses_without_unbound_retry(self) -> None:
        payload = json.loads((FIXTURES / "parse-visual-regions-submit-v1.json").read_text())

        class ChangingSession(FakeSession):
            async def call_tool(self, name, arguments):
                self.calls.append((name, arguments))
                if name == "get_window_state":
                    return SimpleNamespace(
                        isError=False, structuredContent={"capture_id": "capture-submit"}
                    )
                if name == "parse_visual_regions":
                    return SimpleNamespace(isError=False, structuredContent=payload)
                if name == "click":
                    return SimpleNamespace(
                        isError=True,
                        structuredContent={"code": "capture_generation_mismatch"},
                        content=[{"text": "capture changed while provider was deciding"}],
                    )
                raise AssertionError(name)

        session = ChangingSession()
        driver = Driver(session, "jev-test")
        visual = await optional_visual_observation(
            driver,
            7,
            9,
            {"get_window_state", "parse_visual_regions", "click"},
            True,
        )
        page = {
            "target_id": "target",
            "tab_id": "tab",
            "refs": [
                {
                    "role": "textbox",
                    "name": "verification value",
                    "ref": "p1:0",
                    "value": "expected",
                }
            ],
        }
        candidate = validate_choice(
            "submit-form",
            build_candidates(page, "expected", visual, capture_bound_click=True),
            current_capture_id="capture-submit",
        )
        await asyncio.sleep(0)
        with self.assertRaisesRegex(RuntimeError, "click failed"):
            await driver.call(candidate.tool, candidate.arguments)
        click_calls = [call for call in session.calls if call[0] == "click"]
        self.assertEqual(len(click_calls), 1)
        self.assertEqual(click_calls[0][1]["capture_id"], "capture-submit")

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
