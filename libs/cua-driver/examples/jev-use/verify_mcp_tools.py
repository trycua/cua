from __future__ import annotations

import asyncio
import os

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


REQUIRED_TOOLS = {
    "browser_click",
    "browser_navigate",
    "browser_prepare",
    "browser_type",
    "get_browser_state",
    "list_windows",
}
async def verify() -> None:
    params = StdioServerParameters(command=os.getenv("CUA_DRIVER_BIN", "cua-driver"), args=["mcp"])
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            initialized = await session.initialize()
            if not initialized.serverInfo.name:
                raise RuntimeError("MCP initialize returned no server name")
            tools = await session.list_tools()
            names = {tool.name for tool in tools.tools}
            missing = sorted(REQUIRED_TOOLS - names)
            if missing:
                raise RuntimeError(f"MCP tools/list is missing required tools: {missing}")
            click = next((tool for tool in tools.tools if tool.name == "click"), None)
            click_schema = getattr(click, "inputSchema", {}) if click else {}
            click_properties = (
                click_schema.get("properties", {}) if isinstance(click_schema, dict) else {}
            )
            visual_available = (
                "parse_visual_regions" in names and "capture_id" in click_properties
            )
            print(
                f"MCP initialize and tools/list verified {len(REQUIRED_TOOLS)} required tools; "
                f"capture-bound visual adapter available: {visual_available}"
            )


if __name__ == "__main__":
    asyncio.run(verify())
