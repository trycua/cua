"""Write ``mcp.json``: what cua-driver's MCP server says at connect and lists, on the macOS VM it runs on.

Run it inside a macOS VM of the bench image, with the daemon up (the LaunchAgent starts it):

    /opt/agentenv/drivers/cua-driver/venv/bin/python pin_manifest.py

It records the server's ``instructions``, each tool's ``name``, ``description``, ``inputSchema``, ``annotations`` and
``outputSchema``, and each resource's ``uri``, ``name``, ``description`` and ``mimeType``, sorted by name, so a
version bump shows as a reviewable diff."""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

sys.path.insert(0, str(Path(__file__).parent))
from driver import EXECUTABLE, MANIFEST, QUIET, SOCKET  # noqa: E402


async def main() -> None:
    server = StdioServerParameters(
        command=str(EXECUTABLE), args=["mcp", "--socket", str(SOCKET)], env={**os.environ, **QUIET}
    )
    async with stdio_client(server) as (read, write), ClientSession(read, write) as session:
        init = await session.initialize()
        tools = (await session.list_tools()).tools
        resources = (await session.list_resources()).resources
    manifest = {
        "instructions": init.instructions or "",
        "tools": sorted(
            (
                {
                    "name": t.name,
                    "description": t.description or "",
                    "inputSchema": t.inputSchema,
                    "annotations": (
                        t.annotations.model_dump(exclude_none=True) if t.annotations else None
                    ),
                    "outputSchema": t.outputSchema,
                }
                for t in tools
            ),
            key=lambda t: t["name"],
        ),
        "resources": sorted(
            (
                {
                    "uri": str(r.uri),
                    "name": r.name,
                    "description": r.description or "",
                    "mimeType": r.mimeType,
                }
                for r in resources
                if str(r.uri).startswith("skill://cua-driver/")
            ),
            key=lambda r: r["uri"],
        ),
    }
    MANIFEST.write_text(json.dumps(manifest, indent=1, sort_keys=False) + "\n")
    print(
        f"wrote {MANIFEST}: {len(manifest['tools'])} tools, {len(manifest['resources'])} skill files"
    )


if __name__ == "__main__":
    asyncio.run(main())
