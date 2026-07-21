from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path
from typing import Any, Mapping

from cua_driver_client import (
    AsyncCuaDriverClient,
    AsyncStdioMcpTransport,
    CuaDriverClient,
    ClickArgs,
    HotkeyArgs,
    StartSessionArgs,
    StdioMcpTransport,
    ToolResult,
)


FIXTURES = Path(__file__).parents[3] / "contract" / "fixtures"
MCP_FIXTURE = Path(__file__).with_name("mcp_fixture.py")


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

    def test_generated_hotkey_preserves_string_array(self) -> None:
        transport = FakeTransport(fixture("session-success.json"))
        client = CuaDriverClient(transport)
        client.hotkey(HotkeyArgs(["ctrl", "l"], "desktop", session="demo"))
        self.assertEqual(
            transport.calls[0][1],
            {
                "name": "hotkey",
                "arguments": {
                    "keys": ["ctrl", "l"],
                    "scope": "desktop",
                    "session": "demo",
                },
            },
        )

    def test_stdio_transport_executes_initialize_and_desktop_call(self) -> None:
        transport = StdioMcpTransport((sys.executable, str(MCP_FIXTURE)))
        try:
            client = CuaDriverClient(transport)
            result = client.click(ClickArgs(12.5, 20.0, "desktop", session="demo"))
            self.assertEqual(result.structured["name"], "click")
            self.assertEqual(
                result.structured["arguments"],
                {"x": 12.5, "y": 20.0, "scope": "desktop", "session": "demo"},
            )
        finally:
            transport.close()

    def test_stdio_transport_times_out(self) -> None:
        transport = StdioMcpTransport(
            (sys.executable, str(MCP_FIXTURE)), timeout=0.05
        )
        try:
            with self.assertRaisesRegex(TimeoutError, "request timed out"):
                transport.request("test/hang")
        finally:
            transport.close()


class AsyncClientTests(unittest.IsolatedAsyncioTestCase):
    async def test_async_stdio_transport_executes_generated_call(self) -> None:
        transport = AsyncStdioMcpTransport(
            (sys.executable, str(MCP_FIXTURE)), timeout=2.0
        )
        try:
            client = AsyncCuaDriverClient(transport)
            result = await client.click(ClickArgs(4.0, 8.0, "desktop"))
            self.assertEqual(result.structured["name"], "click")
            self.assertEqual(
                result.structured["arguments"],
                {"x": 4.0, "y": 8.0, "scope": "desktop"},
            )
        finally:
            await transport.close()

    async def test_async_stdio_transport_times_out(self) -> None:
        transport = AsyncStdioMcpTransport(
            (sys.executable, str(MCP_FIXTURE)), timeout=0.05
        )
        try:
            with self.assertRaises(TimeoutError):
                await transport.request("test/hang")
        finally:
            await transport.close()


if __name__ == "__main__":
    unittest.main()
