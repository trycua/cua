"""High-level generated cua-driver SDK facade."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

from ._generated import GeneratedAsyncDriverMixin, GeneratedDriverMixin
from .result import ToolResult
from .transport import AsyncStdioMcpTransport, AsyncTransport, StdioMcpTransport, Transport
from .wrapper import get_binary_path


def _bundled_mcp_command() -> tuple[str, str]:
    return (str(get_binary_path()), "mcp")


class CuaDriver(GeneratedDriverMixin):
    def __init__(self, transport: Transport):
        self._transport = transport

    @classmethod
    def stdio(cls, command: Sequence[str] | None = None) -> "CuaDriver":
        resolved = _bundled_mcp_command() if command is None else command
        return cls(StdioMcpTransport(resolved))

    def call_tool(
        self, name: str, arguments: Mapping[str, Any] | None = None
    ) -> ToolResult:
        result = self._transport.request(
            "tools/call", {"name": name, "arguments": dict(arguments or {})}
        )
        return ToolResult.from_mcp(result)

    def list_tools(self) -> Mapping[str, Any]:
        return self._transport.request("tools/list", {})

    def close(self) -> None:
        self._transport.close()

    def __enter__(self) -> "CuaDriver":
        return self

    def __exit__(self, *_error: object) -> None:
        self.close()


class AsyncCuaDriver(GeneratedAsyncDriverMixin):
    def __init__(self, transport: AsyncTransport):
        self._transport = transport

    @classmethod
    def stdio(
        cls, command: Sequence[str] | None = None
    ) -> "AsyncCuaDriver":
        resolved = _bundled_mcp_command() if command is None else command
        return cls(AsyncStdioMcpTransport(resolved))

    async def call_tool(
        self, name: str, arguments: Mapping[str, Any] | None = None
    ) -> ToolResult:
        result = await self._transport.request(
            "tools/call", {"name": name, "arguments": dict(arguments or {})}
        )
        return ToolResult.from_mcp(result)

    async def list_tools(self) -> Mapping[str, Any]:
        return await self._transport.request("tools/list", {})

    async def close(self) -> None:
        await self._transport.close()

    async def __aenter__(self) -> "AsyncCuaDriver":
        return self

    async def __aexit__(self, *_error: object) -> None:
        await self.close()
