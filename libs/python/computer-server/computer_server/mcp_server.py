"""
MCP (Model Context Protocol) server interface for Computer Server.

This module provides an MCP interface to the Computer Server's functionality,
enabling Claude Code and other MCP-compatible tools to directly control computers.

Usage:
    python -m computer_server --mcp
    python -m computer_server --mcp --width 1512 --height 982
    python -m computer_server --mcp --detect-resolution
"""

import base64
import logging
import sys
from typing import Any, Dict, List, Optional, Tuple

from fastmcp import FastMCP
from fastmcp.utilities.types import Image

logger = logging.getLogger(__name__)


# Lazy handler initialization to avoid crashes during import
_handlers = None

# Resolution scaling configuration
_scale_x: float = 1.0
_scale_y: float = 1.0
_target_width: Optional[int] = None
_target_height: Optional[int] = None
_actual_width: Optional[int] = None
_actual_height: Optional[int] = None


def _get_handlers():
    """Lazily initialize handlers on first use."""
    global _handlers
    if _handlers is None:
        from .handlers.factory import HandlerFactory
        _handlers = HandlerFactory.create_handlers()
    return _handlers


async def _detect_actual_resolution_async() -> Tuple[int, int]:
    """Detect the actual screen resolution using the automation handler."""
    try:
        _, automation_handler, _, _, _, _ = _get_handlers()
        result = await automation_handler.get_screen_size()
        return result["width"], result["height"]
    except Exception as e:
        logger.warning(f"Failed to detect resolution via handler: {e}")
        return 1920, 1080  # Default fallback


def _detect_actual_resolution() -> Tuple[int, int]:
    """Detect the actual screen resolution (sync wrapper)."""
    import asyncio
    try:
        loop = asyncio.get_event_loop()
        if loop.is_running():
            # If called from async context, can't use run_until_complete
            # Return default and let async init handle it
            return 1920, 1080
        return loop.run_until_complete(_detect_actual_resolution_async())
    except Exception as e:
        logger.warning(f"Failed to detect resolution: {e}")
        return 1920, 1080  # Default fallback


def _configure_scaling(
    target_width: Optional[int] = None,
    target_height: Optional[int] = None,
    detect_resolution: bool = False,
) -> None:
    """Configure resolution scaling based on target and actual resolution."""
    global _scale_x, _scale_y, _target_width, _target_height, _actual_width, _actual_height

    # Detect actual resolution
    _actual_width, _actual_height = _detect_actual_resolution()
    logger.info(f"Detected actual screen resolution: {_actual_width}x{_actual_height}")

    if target_width and target_height:
        _target_width = target_width
        _target_height = target_height
        _scale_x = _actual_width / target_width
        _scale_y = _actual_height / target_height
        logger.info(
            f"Target resolution: {target_width}x{target_height}, "
            f"Scale factors: x={_scale_x:.3f}, y={_scale_y:.3f}"
        )
    else:
        # No target specified or detect-resolution mode, use actual resolution
        _target_width = _actual_width
        _target_height = _actual_height
        _scale_x = 1.0
        _scale_y = 1.0
        if detect_resolution:
            logger.info("Resolution detection enabled - coordinates will use actual resolution")


def _scale_to_actual(x: int, y: int) -> Tuple[int, int]:
    """Scale coordinates from target resolution to actual resolution."""
    return int(x * _scale_x), int(y * _scale_y)


def _scale_to_target(x: int, y: int) -> Tuple[int, int]:
    """Scale coordinates from actual resolution to target resolution."""
    if _scale_x == 0 or _scale_y == 0:
        return x, y
    return int(x / _scale_x), int(y / _scale_y)


def create_mcp_server() -> FastMCP:
    """
    Create and configure the MCP server with all computer control tools.

    Returns:
        FastMCP: Configured MCP server instance
    """
    mcp = FastMCP(
        name="cua-computer-server",
        instructions="""You are connected to a computer control server that provides low-level
        primitives for interacting with a desktop computer. You can take screenshots, click,
        type text, press keys, scroll, manage windows, read/write files, and run commands.

        Always take a screenshot first to see the current state before performing actions.
        After performing actions, take another screenshot to verify the result."""
    )

    # ============================================================
    # SCREEN & MOUSE ACTIONS
    # ============================================================

    @mcp.tool
    async def computer_screenshot() -> Image:
        """
        Capture a screenshot of the current screen.

        Returns the current screen state as an image. Always call this first
        to see what's on screen before performing any actions.
        """
        from PIL import Image as PILImage
        from io import BytesIO

        _, automation_handler, _, _, _, _ = _get_handlers()
        result = await automation_handler.screenshot()
        image_data = base64.b64decode(result["image_data"])

        # Decode the image
        img = PILImage.open(BytesIO(image_data))

        # If target resolution is set, resize to that
        if _target_width and _target_height and (_scale_x != 1.0 or _scale_y != 1.0):
            img = img.resize((_target_width, _target_height), PILImage.Resampling.LANCZOS)
        else:
            # Resize large images to keep under 1MB limit
            # Max dimension of 1280 is a good balance for visibility and size
            max_dimension = 1280
            width, height = img.size
            if width > max_dimension or height > max_dimension:
                if width > height:
                    new_width = max_dimension
                    new_height = int(height * max_dimension / width)
                else:
                    new_height = max_dimension
                    new_width = int(width * max_dimension / height)
                img = img.resize((new_width, new_height), PILImage.Resampling.LANCZOS)

        # Convert to RGB if necessary (for JPEG compatibility)
        if img.mode in ('RGBA', 'P'):
            img = img.convert('RGB')

        # Use JPEG with quality setting to ensure small file size
        buffered = BytesIO()
        img.save(buffered, format="JPEG", quality=85, optimize=True)
        buffered.seek(0)
        image_data = buffered.getvalue()

        # If still too large, reduce quality further
        if len(image_data) > 900000:  # 900KB threshold
            buffered = BytesIO()
            img.save(buffered, format="JPEG", quality=70, optimize=True)
            buffered.seek(0)
            image_data = buffered.getvalue()

        return Image(data=image_data, format="jpeg")

    @mcp.tool
    async def computer_get_screen_size() -> Dict[str, Any]:
        """
        Get the screen dimensions.

        Returns:
            Dictionary with 'width' and 'height' keys in pixels.
        """
        # Return target resolution if scaling is configured
        if _target_width is not None and _target_height is not None:
            return {"width": int(_target_width), "height": int(_target_height)}
        # Use handler to get screen size
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.get_screen_size()

    @mcp.tool
    async def computer_get_cursor_position() -> Dict[str, Any]:
        """
        Get the current cursor position.

        Returns:
            Dictionary with 'x' and 'y' coordinates.
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        result = await automation_handler.get_cursor_position()
        # Scale to target coordinates if scaling is configured
        if "x" in result and "y" in result:
            scaled_x, scaled_y = _scale_to_target(int(result["x"]), int(result["y"]))
            return {"x": scaled_x, "y": scaled_y}
        return result

    @mcp.tool
    async def computer_click(
        x: int,
        y: int,
        button: str = "left"
    ) -> Dict[str, Any]:
        """
        Click at the specified screen coordinates.

        Args:
            x: X coordinate in pixels
            y: Y coordinate in pixels
            button: Mouse button - 'left', 'right', or 'middle' (default: 'left')
        """
        # Scale from target to actual coordinates
        actual_x, actual_y = _scale_to_actual(x, y)
        _, automation_handler, _, _, _, _ = _get_handlers()
        if button == "left":
            return await automation_handler.left_click(actual_x, actual_y)
        elif button == "right":
            return await automation_handler.right_click(actual_x, actual_y)
        else:
            return await automation_handler.left_click(actual_x, actual_y)

    @mcp.tool
    async def computer_double_click(x: int, y: int) -> Dict[str, Any]:
        """
        Double-click at the specified screen coordinates.

        Args:
            x: X coordinate in pixels
            y: Y coordinate in pixels
        """
        # Scale from target to actual coordinates
        actual_x, actual_y = _scale_to_actual(x, y)
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.double_click(actual_x, actual_y)

    @mcp.tool
    async def computer_move(x: int, y: int) -> Dict[str, Any]:
        """
        Move the cursor to the specified screen coordinates.

        Args:
            x: X coordinate in pixels
            y: Y coordinate in pixels
        """
        # Scale from target to actual coordinates
        actual_x, actual_y = _scale_to_actual(x, y)
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.move_cursor(actual_x, actual_y)

    @mcp.tool
    async def computer_drag(
        start_x: int,
        start_y: int,
        end_x: int,
        end_y: int,
        button: str = "left",
        duration: float = 0.5
    ) -> Dict[str, Any]:
        """
        Drag from start coordinates to end coordinates.

        Args:
            start_x: Starting X coordinate
            start_y: Starting Y coordinate
            end_x: Ending X coordinate
            end_y: Ending Y coordinate
            button: Mouse button to use (default: 'left')
            duration: Duration of the drag in seconds (default: 0.5)
        """
        # Scale from target to actual coordinates
        actual_start_x, actual_start_y = _scale_to_actual(start_x, start_y)
        actual_end_x, actual_end_y = _scale_to_actual(end_x, end_y)
        _, automation_handler, _, _, _, _ = _get_handlers()
        # Move to start position first
        await automation_handler.move_cursor(actual_start_x, actual_start_y)
        return await automation_handler.drag_to(actual_end_x, actual_end_y, button=button, duration=duration)

    @mcp.tool
    async def computer_scroll(
        x: int,
        y: int,
        scroll_x: int = 0,
        scroll_y: int = 0
    ) -> Dict[str, Any]:
        """
        Scroll at the specified position.

        Args:
            x: X coordinate where to scroll
            y: Y coordinate where to scroll
            scroll_x: Horizontal scroll amount (positive = right, negative = left)
            scroll_y: Vertical scroll amount (positive = down, negative = up)
        """
        # Scale from target to actual coordinates
        actual_x, actual_y = _scale_to_actual(x, y)
        _, automation_handler, _, _, _, _ = _get_handlers()
        await automation_handler.move_cursor(actual_x, actual_y)
        return await automation_handler.scroll(scroll_x, scroll_y)

    @mcp.tool
    async def computer_mouse_down(
        x: Optional[int] = None,
        y: Optional[int] = None,
        button: str = "left"
    ) -> Dict[str, Any]:
        """
        Press and hold a mouse button.

        Args:
            x: Optional X coordinate (uses current position if not specified)
            y: Optional Y coordinate (uses current position if not specified)
            button: Mouse button - 'left', 'right', or 'middle' (default: 'left')
        """
        # Scale from target to actual coordinates if provided
        actual_x, actual_y = None, None
        if x is not None and y is not None:
            actual_x, actual_y = _scale_to_actual(x, y)
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.mouse_down(actual_x, actual_y, button=button)

    @mcp.tool
    async def computer_mouse_up(
        x: Optional[int] = None,
        y: Optional[int] = None,
        button: str = "left"
    ) -> Dict[str, Any]:
        """
        Release a mouse button.

        Args:
            x: Optional X coordinate (uses current position if not specified)
            y: Optional Y coordinate (uses current position if not specified)
            button: Mouse button - 'left', 'right', or 'middle' (default: 'left')
        """
        # Scale from target to actual coordinates if provided
        actual_x, actual_y = None, None
        if x is not None and y is not None:
            actual_x, actual_y = _scale_to_actual(x, y)
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.mouse_up(actual_x, actual_y, button=button)

    # ============================================================
    # KEYBOARD ACTIONS
    # ============================================================

    @mcp.tool
    async def computer_type(text: str) -> Dict[str, Any]:
        """
        Type text at the current cursor position.

        Args:
            text: The text to type
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.type_text(text)

    @mcp.tool
    async def computer_press_key(key: str) -> Dict[str, Any]:
        """
        Press a single key.

        Args:
            key: The key to press (e.g., 'enter', 'tab', 'escape', 'a', 'b', etc.)
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.press_key(key)

    @mcp.tool
    async def computer_hotkey(keys: List[str]) -> Dict[str, Any]:
        """
        Press a key combination (hotkey).

        Args:
            keys: List of keys to press together (e.g., ['ctrl', 'c'] for copy,
                  ['cmd', 'v'] for paste on Mac, ['alt', 'tab'] for window switch)
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.hotkey(keys)

    @mcp.tool
    async def computer_key_down(key: str) -> Dict[str, Any]:
        """
        Press and hold a key.

        Args:
            key: The key to hold down
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.key_down(key)

    @mcp.tool
    async def computer_key_up(key: str) -> Dict[str, Any]:
        """
        Release a held key.

        Args:
            key: The key to release
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.key_up(key)

    # ============================================================
    # CLIPBOARD ACTIONS
    # ============================================================

    @mcp.tool
    async def computer_clipboard_get() -> Dict[str, Any]:
        """
        Get the current clipboard content.

        Returns:
            Dictionary with clipboard content.
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.copy_to_clipboard()

    @mcp.tool
    async def computer_clipboard_set(text: str) -> Dict[str, Any]:
        """
        Set the clipboard content.

        Args:
            text: Text to copy to clipboard
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.set_clipboard(text)

    # ============================================================
    # SHELL COMMANDS
    # ============================================================

    @mcp.tool
    async def computer_run_command(command: str) -> Dict[str, Any]:
        """
        Execute a shell command and return the output.

        Args:
            command: The shell command to execute

        Returns:
            Dictionary with 'stdout', 'stderr', and 'returncode' keys.
        """
        _, automation_handler, _, _, _, _ = _get_handlers()
        return await automation_handler.run_command(command)

    # ============================================================
    # FILE SYSTEM OPERATIONS
    # ============================================================

    @mcp.tool
    async def computer_file_read(path: str) -> Dict[str, Any]:
        """
        Read the text content of a file.

        Args:
            path: Path to the file to read

        Returns:
            Dictionary with 'content' key containing the file contents.
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.read_text(path)

    @mcp.tool
    async def computer_file_write(path: str, content: str) -> Dict[str, Any]:
        """
        Write text content to a file.

        Args:
            path: Path to the file to write
            content: Text content to write
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.write_text(path, content)

    @mcp.tool
    async def computer_file_exists(path: str) -> Dict[str, Any]:
        """
        Check if a file exists.

        Args:
            path: Path to check

        Returns:
            Dictionary with 'exists' boolean key.
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.file_exists(path)

    @mcp.tool
    async def computer_directory_exists(path: str) -> Dict[str, Any]:
        """
        Check if a directory exists.

        Args:
            path: Path to check

        Returns:
            Dictionary with 'exists' boolean key.
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.directory_exists(path)

    @mcp.tool
    async def computer_list_directory(path: str) -> Dict[str, Any]:
        """
        List the contents of a directory.

        Args:
            path: Path to the directory

        Returns:
            Dictionary with 'entries' list containing file/folder names.
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.list_dir(path)

    @mcp.tool
    async def computer_create_directory(path: str) -> Dict[str, Any]:
        """
        Create a directory.

        Args:
            path: Path of the directory to create
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.create_dir(path)

    @mcp.tool
    async def computer_delete_file(path: str) -> Dict[str, Any]:
        """
        Delete a file.

        Args:
            path: Path to the file to delete
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.delete_file(path)

    @mcp.tool
    async def computer_delete_directory(path: str) -> Dict[str, Any]:
        """
        Delete a directory.

        Args:
            path: Path to the directory to delete
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.delete_dir(path)

    @mcp.tool
    async def computer_get_file_size(path: str) -> Dict[str, Any]:
        """
        Get the size of a file in bytes.

        Args:
            path: Path to the file

        Returns:
            Dictionary with 'size' key in bytes.
        """
        _, _, _, file_handler, _, _ = _get_handlers()
        return await file_handler.get_file_size(path)

    # ============================================================
    # WINDOW MANAGEMENT
    # ============================================================

    @mcp.tool
    async def computer_open(target: str) -> Dict[str, Any]:
        """
        Open a file or URL with the default application.

        Args:
            target: File path or URL to open
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.open(target)

    @mcp.tool
    async def computer_launch_app(app: str, args: Optional[List[str]] = None) -> Dict[str, Any]:
        """
        Launch an application.

        Args:
            app: Application name or path
            args: Optional list of arguments to pass to the application
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.launch(app, args)

    @mcp.tool
    async def computer_get_active_window() -> Dict[str, Any]:
        """
        Get the currently active window ID.

        Returns:
            Dictionary with 'window_id' key.
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.get_current_window_id()

    @mcp.tool
    async def computer_get_window_name(window_id: str) -> Dict[str, Any]:
        """
        Get the title/name of a window.

        Args:
            window_id: Window identifier

        Returns:
            Dictionary with 'name' key.
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.get_window_name(window_id)

    @mcp.tool
    async def computer_get_window_size(window_id: str) -> Dict[str, Any]:
        """
        Get the size of a window.

        Args:
            window_id: Window identifier

        Returns:
            Dictionary with 'width' and 'height' keys.
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.get_window_size(window_id)

    @mcp.tool
    async def computer_get_window_position(window_id: str) -> Dict[str, Any]:
        """
        Get the position of a window.

        Args:
            window_id: Window identifier

        Returns:
            Dictionary with 'x' and 'y' keys.
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.get_window_position(window_id)

    @mcp.tool
    async def computer_set_window_size(window_id: str, width: int, height: int) -> Dict[str, Any]:
        """
        Set the size of a window.

        Args:
            window_id: Window identifier
            width: New width in pixels
            height: New height in pixels
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.set_window_size(window_id, width, height)

    @mcp.tool
    async def computer_set_window_position(window_id: str, x: int, y: int) -> Dict[str, Any]:
        """
        Set the position of a window.

        Args:
            window_id: Window identifier
            x: New X position
            y: New Y position
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.set_window_position(window_id, x, y)

    @mcp.tool
    async def computer_activate_window(window_id: str) -> Dict[str, Any]:
        """
        Bring a window to the foreground and focus it.

        Args:
            window_id: Window identifier
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.activate_window(window_id)

    @mcp.tool
    async def computer_minimize_window(window_id: str) -> Dict[str, Any]:
        """
        Minimize a window.

        Args:
            window_id: Window identifier
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.minimize_window(window_id)

    @mcp.tool
    async def computer_maximize_window(window_id: str) -> Dict[str, Any]:
        """
        Maximize a window.

        Args:
            window_id: Window identifier
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.maximize_window(window_id)

    @mcp.tool
    async def computer_close_window(window_id: str) -> Dict[str, Any]:
        """
        Close a window.

        Args:
            window_id: Window identifier
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.close_window(window_id)

    @mcp.tool
    async def computer_get_app_windows(app: str) -> Dict[str, Any]:
        """
        Get all windows belonging to an application.

        Args:
            app: Application name or bundle identifier

        Returns:
            Dictionary with 'windows' list.
        """
        _, _, _, _, _, window_handler = _get_handlers()
        return await window_handler.get_application_windows(app)

    # ============================================================
    # DESKTOP ENVIRONMENT
    # ============================================================

    @mcp.tool
    async def computer_get_desktop_environment() -> Dict[str, Any]:
        """
        Get information about the current desktop environment.

        Returns:
            Dictionary with desktop environment details.
        """
        _, _, _, _, desktop_handler, _ = _get_handlers()
        return await desktop_handler.get_desktop_environment()

    @mcp.tool
    async def computer_set_wallpaper(path: str) -> Dict[str, Any]:
        """
        Set the desktop wallpaper.

        Args:
            path: Path to the image file to use as wallpaper
        """
        _, _, _, _, desktop_handler, _ = _get_handlers()
        return await desktop_handler.set_wallpaper(path)

    # ============================================================
    # ACCESSIBILITY
    # ============================================================

    @mcp.tool
    async def computer_get_accessibility_tree() -> Dict[str, Any]:
        """
        Get the accessibility tree of the current window.

        This provides detailed information about UI elements that can be
        useful for finding clickable elements or understanding the UI structure.

        Returns:
            Dictionary with the accessibility tree structure.
        """
        accessibility_handler, _, _, _, _, _ = _get_handlers()
        return await accessibility_handler.get_accessibility_tree()

    @mcp.tool
    async def computer_find_element(
        role: Optional[str] = None,
        title: Optional[str] = None,
        value: Optional[str] = None
    ) -> Dict[str, Any]:
        """
        Find a UI element in the accessibility tree.

        Args:
            role: Element role/type to search for (e.g., 'button', 'textfield')
            title: Element title/label to search for
            value: Element value to search for

        Returns:
            Dictionary with matching element information including position.
        """
        accessibility_handler, _, _, _, _, _ = _get_handlers()
        return await accessibility_handler.find_element(role=role, title=title, value=value)

    return mcp


def run_mcp_server(
    target_width: Optional[int] = None,
    target_height: Optional[int] = None,
    detect_resolution: bool = False,
) -> None:
    """Run the MCP server.

    Args:
        target_width: Target width for screenshots (coordinates will be scaled accordingly)
        target_height: Target height for screenshots (coordinates will be scaled accordingly)
        detect_resolution: If True, auto-detect and log the actual screen resolution
    """
    logger.info("Starting CUA Computer MCP server...")

    # Configure resolution scaling
    _configure_scaling(
        target_width=target_width,
        target_height=target_height,
        detect_resolution=detect_resolution,
    )

    mcp = create_mcp_server()
    mcp.run()


if __name__ == "__main__":
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
        stream=sys.stderr,
    )
    run_mcp_server()
