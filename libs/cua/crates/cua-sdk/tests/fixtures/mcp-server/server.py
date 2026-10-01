"""A trivial FastMCP server (streamable HTTP on :8765/mcp) for cua e2e tests."""
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("cua-e2e-mcp", host="0.0.0.0", port=8765)


@mcp.tool()
def add(a: int, b: int) -> int:
    """Add two integers."""
    return a + b


@mcp.tool()
def echo(text: str) -> str:
    """Echo text."""
    return text


@mcp.resource("mem://greeting")
def greeting() -> str:
    return "hello"


if __name__ == "__main__":
    mcp.run(transport="streamable-http")
