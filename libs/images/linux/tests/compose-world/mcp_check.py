"""Call the world's MCP server with the official MCP client (streamable HTTP):
list its tools, write a file to the workspace, read the server's uptime.

    python mcp_check.py http://127.0.0.1:18000/mcp

Prints one JSON line: the tools, the server's uptime and pid.
"""

import asyncio
import json
import sys

from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client


async def main(url: str) -> None:
    async with streamable_http_client(url) as (read, write, *_):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools = sorted(t.name for t in (await session.list_tools()).tools)
            wrote = await session.call_tool("write_file", {"path": "probe.txt", "content": "ok"})
            up = await session.call_tool("uptime", {})
            print(json.dumps({
                "tools": tools,
                "write": wrote.content[0].text,
                "uptime": json.loads(up.content[0].text),
            }))


asyncio.run(main(sys.argv[1]))
