"""High-level generated cua-driver client facade."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

from ._generated import GeneratedAsyncClientMixin, GeneratedClientMixin
from .result import ToolResult
from .transport import AsyncStdioMcpTransport, AsyncTransport, StdioMcpTransport, Transport


class CuaDriverClient(GeneratedClientMixin):
    def __init__(self, transport: Transport):
        self._transport = transport

    @classmethod
    def stdio(cls, command: Sequence[str] = ("cua-driver", "mcp")) -> "CuaDriverClient":
        return cls(StdioMcpTransport(command))

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

    def __enter__(self) -> "CuaDriverClient":
        return self

    def __exit__(self, *_error: object) -> None:
        self.close()


class AsyncCuaDriverClient(GeneratedAsyncClientMixin):
    def __init__(self, transport: AsyncTransport):
        self._transport = transport

    @classmethod
    def stdio(
        cls, command: Sequence[str] = ("cua-driver", "mcp")
    ) -> "AsyncCuaDriverClient":
        return cls(AsyncStdioMcpTransport(command))

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

    async def __aenter__(self) -> "AsyncCuaDriverClient":
        return self

    async def __aexit__(self, *_error: object) -> None:
        await self.close()
