"""MCP servers inside a sandbox, with the official ``mcp`` Python SDK.

cua does not implement MCP. It only gets bytes to the service: ``sb.mcp_config``
returns the endpoint URL and the headers the route needs (nothing locally; the
Fleet gateway bearer and claim header in the cloud; the daemon bearer through
``cua daemon``). ``sb.mcp`` hands that to the official SDK
(``pip install "cua-sandbox[mcp]"``), so every protocol revision and every
content block the SDK supports works unchanged::

    async with sb.mcp("mcp") as client:
        tools = await client.list_tools()
        result = await client.call_tool("add", {"a": 2, "b": 3})

Any other MCP client (Claude Code, Cursor, the TypeScript SDK) can use
``await sb.mcp_config("mcp")`` directly. Fleet bearers are short-lived: fetch
a fresh config per connection.
"""

from __future__ import annotations

from contextlib import asynccontextmanager
from typing import Any, AsyncIterator, Awaitable, Callable, Dict


def _config_dict(native_config: Any) -> Dict[str, Any]:
    return {
        "url": native_config.url,
        "headers": {h.name: h.value for h in native_config.headers},
    }


async def mcp_config(service: Callable[[], Awaitable[Any]], path: str = "/mcp") -> Dict[str, Any]:
    """``{"url": ..., "headers": {...}}`` of the MCP endpoint at ``path``."""
    native_service = await service()
    return _config_dict(await native_service.mcp_config(path))


@asynccontextmanager
async def connect(config: Dict[str, Any]) -> AsyncIterator[Any]:
    """An initialized official-SDK client for ``config`` (``url``, ``headers``).

    With ``mcp`` 2.x this is an ``mcp.Client`` (protocol negotiated with
    ``server/discover``, falling back to ``initialize``); with 1.x an
    initialized ``mcp.ClientSession``.
    """
    try:
        import mcp  # noqa: PLC0415 - optional dependency
    except ImportError as error:  # pragma: no cover - packaging error
        raise ImportError(
            'sb.mcp needs the official MCP SDK: pip install "cua-sandbox[mcp]"'
        ) from error
    url, headers = config["url"], dict(config.get("headers") or {})
    if hasattr(mcp, "Client"):
        import httpx2  # noqa: PLC0415 - dependency of mcp>=2
        from mcp.client.streamable_http import streamable_http_client  # noqa: PLC0415

        http = httpx2.AsyncClient(headers=headers, timeout=httpx2.Timeout(30.0, read=300.0))
        async with http:
            async with mcp.Client(streamable_http_client(url, http_client=http)) as client:
                yield client
    else:  # mcp 1.x
        from mcp.client.streamable_http import streamablehttp_client  # noqa: PLC0415

        async with streamablehttp_client(url, headers=headers) as (read, write, _):
            async with mcp.ClientSession(read, write) as session:
                await session.initialize()
                yield session


@asynccontextmanager
async def open_mcp(service: Callable[[], Awaitable[Any]], path: str = "/mcp") -> AsyncIterator[Any]:
    """``connect(await mcp_config(service, path))``."""
    async with connect(await mcp_config(service, path)) as client:
        yield client
