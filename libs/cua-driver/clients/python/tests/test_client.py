from __future__ import annotations

import json
import unittest
from pathlib import Path
from typing import Any, Mapping

from cua_driver_client import CuaDriverClient, StartSessionArgs, ToolResult


FIXTURES = Path(__file__).parents[3] / "contract" / "fixtures"


class FakeTransport:
    def __init__(self, response: Mapping[str, Any]):
        self.response = response
        self.calls: list[tuple[str, Mapping[str, Any] | None]] = []

    def request(self, method: str, params: Mapping[str, Any] | None = None) -> Mapping[str, Any]:
        self.calls.append((method, params))
        return self.response

    def close(self) -> None:
        pass


def fixture(name: str) -> Mapping[str, Any]:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))["result"]


class ClientTests(unittest.TestCase):
    def test_generated_session_method_uses_wire_names(self) -> None:
        transport = FakeTransport(fixture("session-success.json"))
        client = CuaDriverClient(transport)
        result = client.start_session(StartSessionArgs("demo", capture_scope="auto"))
        self.assertFalse(result.is_error)
        self.assertEqual(result.structured["session"], "demo")
        self.assertEqual(
            transport.calls,
            [
                (
                    "tools/call",
                    {
                        "name": "start_session",
                        "arguments": {"session": "demo", "capture_scope": "auto"},
                    },
                )
            ],
        )

    def test_normalizes_images_and_refusals(self) -> None:
        image = ToolResult.from_mcp(fixture("image-result.json"))
        self.assertEqual(image.text, "captured")
        self.assertEqual(image.images[0].mime_type, "image/png")
        refused = ToolResult.from_mcp(fixture("tool-refusal.json"))
        self.assertTrue(refused.is_error)
        self.assertEqual(refused.error_code, "foreground_required")


if __name__ == "__main__":
    unittest.main()
