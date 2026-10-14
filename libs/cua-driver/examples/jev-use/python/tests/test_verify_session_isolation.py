"""Offline negative controls for the session-isolation proof's refusal oracle.

The live proof only means something if ``_expect_refusal`` fails when the
Driver accepts a cross-session action. These cases feed it fake MCP results so
a leak (an accepted foreign action) or a refusal for the wrong reason cannot
be reported as isolation.
"""

from __future__ import annotations

import asyncio
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE))
sys.path.insert(0, str(BASE / "python"))

from run import Driver  # noqa: E402
from verify_session_isolation import _expect_refusal  # noqa: E402


class FakeSession:
    def __init__(self, result: SimpleNamespace) -> None:
        self.result = result
        self.calls: list[tuple[str, dict]] = []

    async def call_tool(self, name: str, arguments: dict) -> SimpleNamespace:
        self.calls.append((name, arguments))
        return self.result


def expect(result: SimpleNamespace, code: str = "browser_binding_stale") -> tuple[str, FakeSession]:
    session = FakeSession(result)
    driver = Driver(session, "jev-isolation-b")  # type: ignore[arg-type]
    returned = asyncio.run(
        _expect_refusal(driver, "browser_click", {"ref": "p1:1"}, code)
    )
    return returned, session


class ExpectRefusalTest(unittest.TestCase):
    def test_accepted_foreign_action_is_reported_as_a_leak(self) -> None:
        accepted = SimpleNamespace(
            isError=False,
            structuredContent={"effect": "confirmed", "tool": "browser_click"},
        )
        with self.assertRaisesRegex(RuntimeError, "unexpectedly succeeded"):
            expect(accepted)

    def test_refusal_for_another_reason_is_not_isolation(self) -> None:
        wrong = SimpleNamespace(
            isError=False,
            structuredContent={"effect": "refused", "error": {"code": "element_not_found"}},
        )
        with self.assertRaisesRegex(RuntimeError, "wrong reason"):
            expect(wrong)

    def test_typed_action_result_refusal_is_accepted(self) -> None:
        refused = SimpleNamespace(
            isError=False,
            structuredContent={"effect": "refused", "error": {"code": "browser_binding_stale"}},
        )
        code, session = expect(refused)
        self.assertEqual(code, "browser_binding_stale")
        self.assertEqual(session.calls[0][1]["session"], "jev-isolation-b")

    def test_tool_error_refusal_is_accepted(self) -> None:
        refused = SimpleNamespace(isError=True, structuredContent={"code": "session_ended"})
        code, _ = expect(refused, "session_ended")
        self.assertEqual(code, "session_ended")


if __name__ == "__main__":
    unittest.main()
