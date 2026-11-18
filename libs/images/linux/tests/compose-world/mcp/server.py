"""The world's MCP server (streamable HTTP on :8000/mcp): tools over the
shared workspace volume, so a rollout's agent and grader see the same files."""

import os
import time
from pathlib import Path

from mcp.server.mcpserver import MCPServer

WORKSPACE = Path("/workspace")
STARTED = time.time()
server = MCPServer("world")


@server.tool()
def write_file(path: str, content: str) -> str:
    """Write `content` to `path` under the workspace."""
    target = (WORKSPACE / path).resolve()
    if WORKSPACE.resolve() not in target.parents:
        raise ValueError("path escapes the workspace")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content)
    return f"wrote {len(content)} bytes to {path}"


@server.tool()
def uptime() -> dict:
    """Seconds since this server started, and its pid (a fork keeps both)."""
    return {"seconds": round(time.time() - STARTED, 1), "pid": os.getpid()}


if __name__ == "__main__":
    server.run("streamable-http", host="0.0.0.0", port=8000)
