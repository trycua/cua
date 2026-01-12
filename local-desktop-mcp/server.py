#!/usr/bin/env python3
"""
Local Desktop MCP Server for Claude Code

This MCP server allows Claude Code to directly control your local PC
without needing VMs or cloud services. Simply run the computer server
locally and this MCP server will connect to it.

Usage:
    1. Start the computer server locally:
       python -m computer_server --host localhost --port 8000

    2. Run this MCP server:
       python server.py

    3. Configure Claude Code to use this MCP server (see README.md)
"""

import asyncio
import logging
import os
import platform
import sys
import traceback
from typing import Any, Optional

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    stream=sys.stderr,
)
logger = logging.getLogger("local-desktop-mcp")

try:
    from mcp.server.fastmcp import Context, FastMCP
    from mcp.server.fastmcp.utilities.types import Image
    logger.info("Successfully imported FastMCP")
except ImportError as e:
    logger.error(f"Failed to import FastMCP: {e}")
    logger.error("Install with: pip install 'mcp[cli]'")
    sys.exit(1)

try:
    from computer import Computer
    logger.info("Successfully imported Computer module")
except ImportError as e:
    logger.error(f"Failed to import Computer module: {e}")
    logger.error("Make sure you're in the cua project directory")
    sys.exit(1)


# Detect OS type
def detect_os():
    """Detect the current operating system."""
    system = platform.system().lower()
    if system == "darwin":
        return "macos"
    elif system == "linux":
        return "linux"
    elif system == "windows":
        return "windows"
    else:
        raise ValueError(f"Unsupported OS: {system}")


# Global computer instance
_computer: Optional[Computer] = None


async def get_computer() -> Computer:
    """Get or create the global Computer instance."""
    global _computer
    if _computer is None:
        os_type = detect_os()
        api_host = os.getenv("CUA_API_HOST", "localhost")
        api_port = int(os.getenv("CUA_API_PORT", "8000"))

        logger.info(f"Initializing Computer for {os_type} at {api_host}:{api_port}")

        _computer = Computer(
            os_type=os_type,
            use_host_computer_server=True,
            api_host=api_host,
            api_port=api_port,
            verbosity=logging.INFO,
            telemetry_enabled=False,
        )

        # Initialize the computer interface
        await _computer.__aenter__()
        logger.info("Computer interface initialized")

    return _computer


# Create FastMCP server
mcp = FastMCP(
    name="local-desktop",
    instructions="""Local Desktop Control Server - Allows Claude Code to control your local PC directly.

This server provides tools for:
- Taking screenshots
- Moving and clicking the mouse
- Typing text and pressing keys
- Reading and writing files
- Running shell commands
- Managing windows and applications

All actions are performed on your LOCAL machine, not in a VM.

IMPORTANT: Be careful with commands that could harm your system. Always review actions before executing them.
""",
)


@mcp.tool()
async def screenshot(ctx: Context) -> Image:
    """
    Take a screenshot of your local desktop and return the image.

    Returns:
        Image: PNG screenshot of the current desktop
    """
    try:
        computer = await get_computer()
        screenshot_data = await computer.interface.screenshot()
        return Image(format="png", data=screenshot_data)
    except Exception as e:
        error_msg = f"Error taking screenshot: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def mouse_move(ctx: Context, x: int, y: int) -> str:
    """
    Move the mouse cursor to the specified coordinates.

    Args:
        x: X coordinate (pixels from left)
        y: Y coordinate (pixels from top)

    Returns:
        str: Success message
    """
    try:
        computer = await get_computer()
        await computer.interface.move_cursor(x, y)
        return f"Moved mouse to ({x}, {y})"
    except Exception as e:
        error_msg = f"Error moving mouse: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def mouse_click(
    ctx: Context,
    button: str = "left",
    x: Optional[int] = None,
    y: Optional[int] = None
) -> str:
    """
    Click the mouse button at the current position or specified coordinates.

    Args:
        button: Mouse button to click ('left', 'right', 'middle')
        x: Optional X coordinate to move to before clicking
        y: Optional Y coordinate to move to before clicking

    Returns:
        str: Success message
    """
    try:
        computer = await get_computer()

        # Move to position if specified
        if x is not None and y is not None:
            await computer.interface.move_cursor(x, y)

        # Click the appropriate button
        if button == "left":
            await computer.interface.left_click()
        elif button == "right":
            await computer.interface.right_click()
        elif button == "middle":
            await computer.interface.middle_click()
        else:
            raise ValueError(f"Invalid button: {button}")

        pos_str = f" at ({x}, {y})" if x is not None and y is not None else ""
        return f"Clicked {button} button{pos_str}"
    except Exception as e:
        error_msg = f"Error clicking mouse: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def double_click(ctx: Context, x: Optional[int] = None, y: Optional[int] = None) -> str:
    """
    Double-click the left mouse button at the current position or specified coordinates.

    Args:
        x: Optional X coordinate to move to before double-clicking
        y: Optional Y coordinate to move to before double-clicking

    Returns:
        str: Success message
    """
    try:
        computer = await get_computer()

        # Move to position if specified
        if x is not None and y is not None:
            await computer.interface.move_cursor(x, y)

        await computer.interface.double_click()

        pos_str = f" at ({x}, {y})" if x is not None and y is not None else ""
        return f"Double-clicked{pos_str}"
    except Exception as e:
        error_msg = f"Error double-clicking: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def type_text(ctx: Context, text: str) -> str:
    """
    Type the specified text at the current cursor position.

    Args:
        text: Text to type

    Returns:
        str: Success message
    """
    try:
        computer = await get_computer()
        await computer.interface.type_text(text)
        # Truncate text in response if it's too long
        display_text = text if len(text) <= 50 else f"{text[:50]}..."
        return f"Typed: {display_text}"
    except Exception as e:
        error_msg = f"Error typing text: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def press_key(ctx: Context, key: str) -> str:
    """
    Press a keyboard key or key combination.

    Args:
        key: Key to press (e.g., 'enter', 'tab', 'escape', 'ctrl+c', 'cmd+v')

    Returns:
        str: Success message
    """
    try:
        computer = await get_computer()

        # Handle key combinations (e.g., 'ctrl+c')
        if '+' in key:
            await computer.interface.hotkey(*key.split('+'))
        else:
            await computer.interface.press_key(key)

        return f"Pressed key: {key}"
    except Exception as e:
        error_msg = f"Error pressing key: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def scroll(ctx: Context, amount: int, direction: str = "down") -> str:
    """
    Scroll the mouse wheel.

    Args:
        amount: Number of scroll units (positive for up/right, negative for down/left)
        direction: Scroll direction ('up', 'down', 'left', 'right')

    Returns:
        str: Success message
    """
    try:
        computer = await get_computer()

        # Normalize direction
        direction = direction.lower()
        if direction in ["up", "down"]:
            # Vertical scrolling
            scroll_amount = amount if direction == "up" else -amount
            await computer.interface.scroll(0, scroll_amount)
        elif direction in ["left", "right"]:
            # Horizontal scrolling
            scroll_amount = amount if direction == "right" else -amount
            await computer.interface.scroll(scroll_amount, 0)
        else:
            raise ValueError(f"Invalid direction: {direction}")

        return f"Scrolled {direction} by {amount} units"
    except Exception as e:
        error_msg = f"Error scrolling: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def run_command(ctx: Context, command: str, timeout: int = 30) -> str:
    """
    Run a shell command on the local system.

    WARNING: This executes commands directly on your system. Be careful!

    Args:
        command: Shell command to execute
        timeout: Timeout in seconds (default: 30)

    Returns:
        str: Command output (stdout + stderr)
    """
    try:
        computer = await get_computer()
        result = await computer.interface.run_command(command, timeout=timeout)

        # Extract output from result
        if isinstance(result, dict):
            output = result.get("output", str(result))
        else:
            output = str(result)

        return f"Command: {command}\n\nOutput:\n{output}"
    except Exception as e:
        error_msg = f"Error running command: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def read_file(ctx: Context, file_path: str) -> str:
    """
    Read the contents of a text file.

    Args:
        file_path: Path to the file to read

    Returns:
        str: File contents
    """
    try:
        computer = await get_computer()
        content = await computer.interface.read_text(file_path)
        return f"File: {file_path}\n\n{content}"
    except Exception as e:
        error_msg = f"Error reading file: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def write_file(ctx: Context, file_path: str, content: str) -> str:
    """
    Write content to a file (overwrites existing file).

    Args:
        file_path: Path to the file to write
        content: Content to write

    Returns:
        str: Success message
    """
    try:
        computer = await get_computer()
        await computer.interface.write_text(file_path, content)
        return f"Wrote {len(content)} characters to {file_path}"
    except Exception as e:
        error_msg = f"Error writing file: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def get_cursor_position(ctx: Context) -> str:
    """
    Get the current mouse cursor position.

    Returns:
        str: Current cursor position
    """
    try:
        computer = await get_computer()
        pos = await computer.interface.get_cursor_position()
        return f"Cursor position: {pos}"
    except Exception as e:
        error_msg = f"Error getting cursor position: {str(e)}"
        logger.error(error_msg)
        raise


@mcp.tool()
async def get_screen_size(ctx: Context) -> str:
    """
    Get the screen dimensions.

    Returns:
        str: Screen size (width x height)
    """
    try:
        computer = await get_computer()
        size = await computer.interface.get_screen_size()
        return f"Screen size: {size}"
    except Exception as e:
        error_msg = f"Error getting screen size: {str(e)}"
        logger.error(error_msg)
        raise


async def cleanup():
    """Cleanup resources on shutdown."""
    global _computer
    if _computer is not None:
        try:
            await _computer.__aexit__(None, None, None)
            logger.info("Computer interface closed")
        except Exception as e:
            logger.error(f"Error closing computer interface: {e}")
        finally:
            _computer = None


async def run_server():
    """Run the MCP server with proper lifecycle management."""
    try:
        logger.info("Starting Local Desktop MCP Server...")
        logger.info(f"OS: {detect_os()}")
        logger.info(f"Target: {os.getenv('CUA_API_HOST', 'localhost')}:{os.getenv('CUA_API_PORT', '8000')}")
        logger.info("=" * 60)
        logger.info("Make sure the computer server is running:")
        logger.info("  python -m computer_server --host localhost --port 8000")
        logger.info("=" * 60)

        # Run the server
        await mcp.run_stdio_async()
    except Exception as e:
        logger.error(f"Error running server: {e}")
        traceback.print_exc(file=sys.stderr)
        raise
    finally:
        await cleanup()


def main():
    """Main entry point."""
    try:
        asyncio.run(run_server())
    except KeyboardInterrupt:
        logger.info("Server interrupted by user")
    except Exception as e:
        logger.error(f"Fatal error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
