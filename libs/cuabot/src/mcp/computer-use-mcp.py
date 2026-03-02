#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["fastmcp", "httpx"]
# ///
"""
Computer-Use MCP Server (self-contained with uv)
Runs inside the container and connects to host's cuabotd HTTP server
Provides screenshot and input tools for Claude coding agents
"""

import os
import time

import httpx
from fastmcp import FastMCP
from fastmcp.utilities.types import Image

# Connect to host's cuabotd server
HOST_URL = os.environ.get("CUABOT_HOST", "http://host.docker.internal:7842")
TELEMETRY_ENABLED = os.environ.get("CUABOT_TELEMETRY", "false").lower() == "true"
STARTING_ERROR = "cuabotd is still starting"

# Overlay cursor socket
OVERLAY_SOCKET = "/tmp/cuabot-overlay-cursor.sock"

server = FastMCP("computer-use")

# HTTP client for cuabotd
client = httpx.Client(base_url=HOST_URL, timeout=30.0)

# Separate client for telemetry with short timeout
telemetry_client = httpx.Client(base_url=HOST_URL, timeout=1.0)


def log_mcp_tool_call(tool_name: str, tool_args: dict) -> None:
    """Send MCP tool call telemetry to cuabotd"""
    if not TELEMETRY_ENABLED:
        return
    try:
        # Filter out large data (like screenshots) from args
        filtered_args = {k: v for k, v in tool_args.items() if not isinstance(v, bytes)}
        event = {
            "type": "mcp_tool_call",
            "timestamp": int(time.time() * 1000),
            "tool_name": tool_name,
            "tool_args": filtered_args,
        }
        telemetry_client.post("/telemetry", json=event)
    except Exception:
        pass  # Silently ignore telemetry errors


def request(endpoint: str, body: dict | None = None) -> dict:
    """Make HTTP request to cuabotd server with retry on starting error."""
    max_retries = 120  # 2 minutes max wait
    retry_delay = 1.0  # 1 second between retries

    for attempt in range(max_retries):
        res = client.post(f"/{endpoint}", json=body)
        data = res.json()
        if "error" in data:
            # If server is still starting, wait and retry
            if data["error"] == STARTING_ERROR:
                if attempt == 0:
                    print("Waiting for cuabotd to finish starting...", flush=True)
                time.sleep(retry_delay)
                continue
            raise Exception(data["error"])
        return data

    raise Exception("Timed out waiting for cuabotd to start")


@server.tool()
def screenshot(save_path: str | None = None) -> Image:
    """Take a screenshot of the display. Returns JPEG image.

    Args:
        save_path: Optional path to save the screenshot (e.g., "/tmp/screenshot.jpg")
    """
    import base64

    log_mcp_tool_call("screenshot", {"save_path": save_path})
    result = request("screenshot")
    image_bytes = base64.b64decode(result["image"])

    if save_path:
        with open(save_path, "wb") as f:
            f.write(image_bytes)

    return Image(data=image_bytes, format="jpeg")


@server.tool()
def click(x: int, y: int, button: str = "left") -> str:
    """Click at x,y coordinates.

    Args:
        x: X coordinate
        y: Y coordinate
        button: Mouse button - left, right, or middle (default: left)
    """
    log_mcp_tool_call("click", {"x": x, "y": y, "button": button})
    request("click", {"x": x, "y": y, "button": button})
    return "Clicked"


@server.tool()
def double_click(x: int, y: int) -> str:
    """Double-click at x,y coordinates.

    Args:
        x: X coordinate
        y: Y coordinate
    """
    log_mcp_tool_call("double_click", {"x": x, "y": y})
    request("doubleClick", {"x": x, "y": y})
    return "Double-clicked"


@server.tool()
def type_text(text: str, delay: int = 50) -> str:
    """Type text using the keyboard.

    Args:
        text: Text to type
        delay: Delay between keystrokes in ms (default: 50)
    """
    log_mcp_tool_call("type_text", {"text": text, "delay": delay})
    request("type", {"text": text, "delay": delay})
    return "Typed"


@server.tool()
def mouse_move(x: int, y: int) -> str:
    """Move mouse to x,y coordinates.

    Args:
        x: X coordinate
        y: Y coordinate
    """
    log_mcp_tool_call("mouse_move", {"x": x, "y": y})
    request("mouseMove", {"x": x, "y": y})
    return "Mouse moved"


@server.tool()
def mouse_down(x: int, y: int, button: str = "left") -> str:
    """Press mouse button down at x,y.

    Args:
        x: X coordinate
        y: Y coordinate
        button: Mouse button - left, right, or middle (default: left)
    """
    log_mcp_tool_call("mouse_down", {"x": x, "y": y, "button": button})
    request("mouseDown", {"x": x, "y": y, "button": button})
    return "Mouse down"


@server.tool()
def mouse_up(x: int, y: int, button: str = "left") -> str:
    """Release mouse button at x,y.

    Args:
        x: X coordinate
        y: Y coordinate
        button: Mouse button - left, right, or middle (default: left)
    """
    log_mcp_tool_call("mouse_up", {"x": x, "y": y, "button": button})
    request("mouseUp", {"x": x, "y": y, "button": button})
    return "Mouse up"


@server.tool()
def scroll(x: int, y: int, delta_x: int, delta_y: int) -> str:
    """Scroll at x,y coordinates.

    Args:
        x: X coordinate
        y: Y coordinate
        delta_x: Horizontal scroll amount
        delta_y: Vertical scroll amount (negative = up, positive = down)
    """
    log_mcp_tool_call("scroll", {"x": x, "y": y, "delta_x": delta_x, "delta_y": delta_y})
    request("scroll", {"x": x, "y": y, "deltaX": delta_x, "deltaY": delta_y})
    return "Scrolled"


@server.tool()
def key_down(key: str) -> str:
    """Press a key down (without releasing).

    Args:
        key: Key to press (e.g., 'Shift', 'Control', 'a', 'Enter')
    """
    log_mcp_tool_call("key_down", {"key": key})
    request("keyDown", {"key": key})
    return "Key down"


@server.tool()
def key_up(key: str) -> str:
    """Release a key.

    Args:
        key: Key to release
    """
    log_mcp_tool_call("key_up", {"key": key})
    request("keyUp", {"key": key})
    return "Key up"


@server.tool()
def key_press(key: str) -> str:
    """Press and release a key.

    Args:
        key: Key to press (e.g., 'Enter', 'Tab', 'Escape', 'F1')
    """
    log_mcp_tool_call("key_press", {"key": key})
    request("keyPress", {"key": key})
    return "Key pressed"


@server.tool()
def drag(from_x: int, from_y: int, to_x: int, to_y: int) -> str:
    """Drag from one point to another.

    Args:
        from_x: Start X coordinate
        from_y: Start Y coordinate
        to_x: End X coordinate
        to_y: End Y coordinate
    """
    log_mcp_tool_call("drag", {"from_x": from_x, "from_y": from_y, "to_x": to_x, "to_y": to_y})
    request("drag", {"fromX": from_x, "fromY": from_y, "toX": to_x, "toY": to_y})
    return "Dragged"


if __name__ == "__main__":
    server.run()
