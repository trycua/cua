import asyncio
import json
import time
from typing import Any, Dict, List, Optional, Tuple

import aiohttp
import websockets
from PIL import Image

from ..logger import Logger, LogLevel
from ..utils import (
    bytes_to_image,
    decode_base64_image,
    draw_box,
    encode_base64_image,
    resize_image,
)
from .base import BaseComputerInterface
from .models import CommandResult, Key, KeyType, MouseButton


class GenericComputerInterface(BaseComputerInterface):
    """Generic interface with common functionality for all supported platforms (Windows, Linux, macOS)."""

    def __init__(
        self,
        ip_address: str,
        username: str = "lume",
        password: str = "lume",
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
        logger_name: str = "computer.interface.generic",
        api_port: Optional[int] = None,
    ):
        super().__init__(ip_address, username, password, api_key, vm_name)
        self._ws = None
        self._reconnect_task = None
        self._closed = False
        self._last_ping = 0
        self._ping_interval = 5  # Send ping every 5 seconds
        self._ping_timeout = 120  # Wait 120 seconds for pong response
        self._reconnect_delay = 1  # Start with 1 second delay
        self._max_reconnect_delay = 30  # Maximum delay between reconnection attempts
        self._log_connection_attempts = True  # Flag to control connection attempt logging
        self._authenticated = False  # Track authentication status
        self._recv_lock = asyncio.Lock()  # Lock to ensure only one recv at a time

        # Set logger name for the interface
        self.logger = Logger(logger_name, LogLevel.NORMAL)

        # Store custom ports
        self._api_port = api_port

        # Optional default delay time between commands (in seconds)
        self.delay = 0.0

    async def _handle_delay(self, delay: Optional[float] = None):
        """Handle delay between commands using async sleep.

        Args:
            delay: Optional delay in seconds. If None, uses self.delay.
        """
        if delay is not None:
            if isinstance(delay, float) or isinstance(delay, int) and delay > 0:
                await asyncio.sleep(delay)
        elif isinstance(self.delay, float) or isinstance(self.delay, int) and self.delay > 0:
            await asyncio.sleep(self.delay)

    @property
    def ws_uri(self) -> str:
        """Get the WebSocket URI using the current IP address.

        Returns:
            WebSocket URI for the Computer API Server
        """
        protocol = "wss" if self.api_key else "ws"
        # Use custom API port if provided, otherwise use defaults based on API key
        port = (
            str(self._api_port)
            if self._api_port is not None
            else ("8443" if self.api_key else "8000")
        )
        return f"{protocol}://{self.ip_address}:{port}/ws"

    @property
    def rest_uri(self) -> str:
        """Get the REST URI using the current IP address.

        Returns:
            REST URI for the Computer API Server
        """
        protocol = "https" if self.api_key else "http"
        # Use custom API port if provided, otherwise use defaults based on API key
        port = (
            str(self._api_port)
            if self._api_port is not None
            else ("8443" if self.api_key else "8000")
        )
        return f"{protocol}://{self.ip_address}:{port}/cmd"

    # Mouse actions
    async def mouse_down(
        self,
        x: Optional[int] = None,
        y: Optional[int] = None,
        button: str = "left",
        delay: Optional[float] = None,
    ) -> None:
        await self._send_command("mouse_down", {"x": x, "y": y, "button": button})
        await self._handle_delay(delay)

    async def mouse_up(
        self,
        x: Optional[int] = None,
        y: Optional[int] = None,
        button: str = "left",
        delay: Optional[float] = None,
    ) -> None:
        await self._send_command("mouse_up", {"x": x, "y": y, "button": button})
        await self._handle_delay(delay)

    async def left_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        await self._send_command("left_click", {"x": x, "y": y})
        await self._handle_delay(delay)

    async def right_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        await self._send_command("right_click", {"x": x, "y": y})
        await self._handle_delay(delay)

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        await self._send_command("double_click", {"x": x, "y": y})
        await self._handle_delay(delay)

    async def move_cursor(self, x: int, y: int, delay: Optional[float] = None) -> None:
        await self._send_command("move_cursor", {"x": x, "y": y})
        await self._handle_delay(delay)

    async def drag_to(
        self,
        x: int,
        y: int,
        button: "MouseButton" = "left",
        duration: float = 0.5,
        delay: Optional[float] = None,
    ) -> None:
        await self._send_command(
            "drag_to", {"x": x, "y": y, "button": button, "duration": duration}
        )
        await self._handle_delay(delay)

    async def drag(
        self,
        path: List[Tuple[int, int]],
        button: "MouseButton" = "left",
        duration: float = 0.5,
        delay: Optional[float] = None,
    ) -> None:
        await self._send_command("drag", {"path": path, "button": button, "duration": duration})
        await self._handle_delay(delay)

    # Keyboard Actions
    async def key_down(self, key: "KeyType", delay: Optional[float] = None) -> None:
        await self._send_command("key_down", {"key": key})
        await self._handle_delay(delay)

    async def key_up(self, key: "KeyType", delay: Optional[float] = None) -> None:
        await self._send_command("key_up", {"key": key})
        await self._handle_delay(delay)

    async def type_text(self, text: str, delay: Optional[float] = None) -> None:
        await self._send_command("type_text", {"text": text})
        await self._handle_delay(delay)

    async def press(self, key: "KeyType", delay: Optional[float] = None) -> None:
        """Press a single key.

        Args:
            key: The key to press. Can be any of:
                - A Key enum value (recommended), e.g. Key.PAGE_DOWN
                - A direct key value string, e.g. 'pagedown'
                - A single character string, e.g. 'a'

        Examples:
            ```python
            # Using enum (recommended)
            await interface.press(Key.PAGE_DOWN)
            await interface.press(Key.ENTER)

            # Using direct values
            await interface.press('pagedown')
            await interface.press('enter')

            # Using single characters
            await interface.press('a')
            ```

        Raises:
            ValueError: If the key type is invalid or the key is not recognized
        """
        if isinstance(key, Key):
            actual_key = key.value
        elif isinstance(key, str):
            # Try to convert to enum if it matches a known key
            key_or_enum = Key.from_string(key)
            actual_key = key_or_enum.value if isinstance(key_or_enum, Key) else key_or_enum
        else:
            raise ValueError(f"Invalid key type: {type(key)}. Must be Key enum or string.")

        await self._send_command("press_key", {"key": actual_key})
        await self._handle_delay(delay)

    async def press_key(self, key: "KeyType", delay: Optional[float] = None) -> None:
        """DEPRECATED: Use press() instead.

        This method is kept for backward compatibility but will be removed in a future version.
        Please use the press() method instead.
        """
        await self.press(key, delay)

    async def hotkey(self, *keys: "KeyType", delay: Optional[float] = None) -> None:
        """Press multiple keys simultaneously.

        Args:
            *keys: Multiple keys to press simultaneously. Each key can be any of:
                - A Key enum value (recommended), e.g. Key.COMMAND
                - A direct key value string, e.g. 'command'
                - A single character string, e.g. 'a'

        Examples:
            ```python
            # Using enums (recommended)
            await interface.hotkey(Key.COMMAND, Key.C)  # Copy
            await interface.hotkey(Key.COMMAND, Key.V)  # Paste

            # Using mixed formats
            await interface.hotkey(Key.COMMAND, 'a')  # Select all
            ```

        Raises:
            ValueError: If any key type is invalid or not recognized
        """
        actual_keys = []
        for key in keys:
            if isinstance(key, Key):
                actual_keys.append(key.value)
            elif isinstance(key, str):
                # Try to convert to enum if it matches a known key
                key_or_enum = Key.from_string(key)
                actual_keys.append(
                    key_or_enum.value if isinstance(key_or_enum, Key) else key_or_enum
                )
            else:
                raise ValueError(f"Invalid key type: {type(key)}. Must be Key enum or string.")

        await self._send_command("hotkey", {"keys": actual_keys})
        await self._handle_delay(delay)

    # Scrolling Actions
    async def scroll(self, x: int, y: int, delay: Optional[float] = None) -> None:
        await self._send_command("scroll", {"x": x, "y": y})
        await self._handle_delay(delay)

    async def scroll_down(self, clicks: int = 1, delay: Optional[float] = None) -> None:
        await self._send_command("scroll_down", {"clicks": clicks})
        await self._handle_delay(delay)

    async def scroll_up(self, clicks: int = 1, delay: Optional[float] = None) -> None:
        await self._send_command("scroll_up", {"clicks": clicks})
        await self._handle_delay(delay)

    # Screen actions
    async def screenshot(
        self,
        boxes: Optional[List[Tuple[int, int, int, int]]] = None,
        box_color: str = "#FF0000",
        box_thickness: int = 2,
        scale_factor: float = 1.0,
    ) -> bytes:
        """Take a screenshot with optional box drawing and scaling.

        Args:
            boxes: Optional list of (x, y, width, height) tuples defining boxes to draw in screen coordinates
            box_color: Color of the boxes in hex format (default: "#FF0000" red)
            box_thickness: Thickness of the box borders in pixels (default: 2)
            scale_factor: Factor to scale the final image by (default: 1.0)
                         Use > 1.0 to enlarge, < 1.0 to shrink (e.g., 0.5 for half size, 2.0 for double)

        Returns:
            bytes: The screenshot image data, optionally with boxes drawn on it and scaled
        """
        result = await self._send_command("screenshot")
        if not result.get("image_data"):
            raise RuntimeError("Failed to take screenshot, no image data received from server")

        screenshot = decode_base64_image(result["image_data"])

        if boxes:
            # Get the natural scaling between screen and screenshot
            screen_size = await self.get_screen_size()
            screenshot_width, screenshot_height = bytes_to_image(screenshot).size
            width_scale = screenshot_width / screen_size["width"]
            height_scale = screenshot_height / screen_size["height"]

            # Scale box coordinates from screen space to screenshot space
            for box in boxes:
                scaled_box = (
                    int(box[0] * width_scale),  # x
                    int(box[1] * height_scale),  # y
                    int(box[2] * width_scale),  # width
                    int(box[3] * height_scale),  # height
                )
                screenshot = draw_box(
                    screenshot,
                    x=scaled_box[0],
                    y=scaled_box[1],
                    width=scaled_box[2],
                    height=scaled_box[3],
                    color=box_color,
                    thickness=box_thickness,
                )

        if scale_factor != 1.0:
            screenshot = resize_image(screenshot, scale_factor)

        return screenshot

    async def get_screen_size(self) -> Dict[str, int]:
        result = await self._send_command("get_screen_size")
        if result["success"] and result["size"]:
            return result["size"]
        raise RuntimeError("Failed to get screen size")

    async def get_cursor_position(self) -> Dict[str, int]:
        result = await self._send_command("get_cursor_position")
        if result["success"] and result["position"]:
            return result["position"]
        raise RuntimeError("Failed to get cursor position")

    # Clipboard Actions
    async def copy_to_clipboard(self) -> str:
        result = await self._send_command("copy_to_clipboard")
        if result["success"] and result["content"]:
            return result["content"]
        raise RuntimeError("Failed to get clipboard content")

    async def set_clipboard(self, text: str) -> None:
        await self._send_command("set_clipboard", {"text": text})

    # File Operations
    async def _write_bytes_chunked(
        self, path: str, content: bytes, append: bool = False, chunk_size: int = 1024 * 1024
    ) -> None:
        """Write large files in chunks to avoid memory issues."""
        total_size = len(content)
        current_offset = 0

        while current_offset < total_size:
            chunk_end = min(current_offset + chunk_size, total_size)
            chunk_data = content[current_offset:chunk_end]

            # First chunk uses the original append flag, subsequent chunks always append
            chunk_append = append if current_offset == 0 else True

            result = await self._send_command(
                "write_bytes",
                {
                    "path": path,
                    "content_b64": encode_base64_image(chunk_data),
                    "append": chunk_append,
                },
            )

            if not result.get("success", False):
                raise RuntimeError(result.get("error", "Failed to write file chunk"))

            current_offset = chunk_end

    async def write_bytes(self, path: str, content: bytes, append: bool = False) -> None:
        # For large files, use chunked writing
        if len(content) > 5 * 1024 * 1024:  # 5MB threshold
            await self._write_bytes_chunked(path, content, append)
            return

        result = await self._send_command(
            "write_bytes",
            {"path": path, "content_b64": encode_base64_image(content), "append": append},
        )
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to write file"))

    async def _read_bytes_chunked(
        self, path: str, offset: int, total_length: int, chunk_size: int = 1024 * 1024
    ) -> bytes:
        """Read large files in chunks to avoid memory issues."""
        chunks = []
        current_offset = offset
        remaining = total_length

        while remaining > 0:
            read_size = min(chunk_size, remaining)
            result = await self._send_command(
                "read_bytes", {"path": path, "offset": current_offset, "length": read_size}
            )

            if not result.get("success", False):
                raise RuntimeError(result.get("error", "Failed to read file chunk"))

            content_b64 = result.get("content_b64", "")
            chunk_data = decode_base64_image(content_b64)
            chunks.append(chunk_data)

            current_offset += read_size
            remaining -= read_size

        return b"".join(chunks)

    async def read_bytes(self, path: str, offset: int = 0, length: Optional[int] = None) -> bytes:
        # For large files, use chunked reading
        if length is None:
            # Get file size first to determine if we need chunking
            file_size = await self.get_file_size(path)
            # If file is larger than 5MB, read in chunks
            if file_size > 5 * 1024 * 1024:  # 5MB threshold
                return await self._read_bytes_chunked(
                    path, offset, file_size - offset if offset > 0 else file_size
                )

        result = await self._send_command(
            "read_bytes", {"path": path, "offset": offset, "length": length}
        )
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to read file"))
        content_b64 = result.get("content_b64", "")
        return decode_base64_image(content_b64)

    async def read_text(self, path: str, encoding: str = "utf-8") -> str:
        """Read text from a file with specified encoding.

        Args:
            path: Path to the file to read
            encoding: Text encoding to use (default: 'utf-8')

        Returns:
            str: The decoded text content of the file
        """
        content_bytes = await self.read_bytes(path)
        return content_bytes.decode(encoding)

    async def write_text(
        self, path: str, content: str, encoding: str = "utf-8", append: bool = False
    ) -> None:
        """Write text to a file with specified encoding.

        Args:
            path: Path to the file to write
            content: Text content to write
            encoding: Text encoding to use (default: 'utf-8')
            append: Whether to append to the file instead of overwriting
        """
        content_bytes = content.encode(encoding)
        await self.write_bytes(path, content_bytes, append)

    async def get_file_size(self, path: str) -> int:
        result = await self._send_command("get_file_size", {"path": path})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get file size"))
        return result.get("size", 0)

    async def file_exists(self, path: str) -> bool:
        result = await self._send_command("file_exists", {"path": path})
        return result.get("exists", False)

    async def directory_exists(self, path: str) -> bool:
        result = await self._send_command("directory_exists", {"path": path})
        return result.get("exists", False)

    async def create_dir(self, path: str) -> None:
        result = await self._send_command("create_dir", {"path": path})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to create directory"))

    async def delete_file(self, path: str) -> None:
        result = await self._send_command("delete_file", {"path": path})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to delete file"))

    async def delete_dir(self, path: str) -> None:
        result = await self._send_command("delete_dir", {"path": path})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to delete directory"))

    async def list_dir(self, path: str) -> list[str]:
        result = await self._send_command("list_dir", {"path": path})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to list directory"))
        return result.get("files", [])

    # Desktop actions
    async def get_desktop_environment(self) -> str:
        result = await self._send_command("get_desktop_environment")
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get desktop environment"))
        return result.get("environment", "unknown")

    async def set_wallpaper(self, path: str) -> None:
        result = await self._send_command("set_wallpaper", {"path": path})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to set wallpaper"))

    # Window management
    async def open(self, target: str) -> None:
        result = await self._send_command("open", {"target": target})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to open target"))

    async def launch(self, app: str, args: list[str] | None = None) -> int | None:
        payload: dict[str, object] = {"app": app}
        if args is not None:
            payload["args"] = args
        result = await self._send_command("launch", payload)
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to launch application"))
        return result.get("pid")  # type: ignore[return-value]

    async def get_current_window_id(self) -> int | str:
        result = await self._send_command("get_current_window_id")
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get current window id"))
        return result["window_id"]  # type: ignore[return-value]

    async def get_application_windows(self, app: str) -> list[int | str]:
        result = await self._send_command("get_application_windows", {"app": app})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get application windows"))
        return list(result.get("windows", []))  # type: ignore[return-value]

    async def get_window_name(self, window_id: int | str) -> str:
        result = await self._send_command("get_window_name", {"window_id": window_id})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get window name"))
        return result.get("name", "")  # type: ignore[return-value]

    async def get_window_size(self, window_id: int | str) -> tuple[int, int]:
        result = await self._send_command("get_window_size", {"window_id": window_id})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get window size"))
        return int(result.get("width", 0)), int(result.get("height", 0))

    async def get_window_position(self, window_id: int | str) -> tuple[int, int]:
        result = await self._send_command("get_window_position", {"window_id": window_id})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get window position"))
        return int(result.get("x", 0)), int(result.get("y", 0))

    async def set_window_size(self, window_id: int | str, width: int, height: int) -> None:
        result = await self._send_command(
            "set_window_size", {"window_id": window_id, "width": width, "height": height}
        )
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to set window size"))

    async def set_window_position(self, window_id: int | str, x: int, y: int) -> None:
        result = await self._send_command(
            "set_window_position", {"window_id": window_id, "x": x, "y": y}
        )
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to set window position"))

    async def maximize_window(self, window_id: int | str) -> None:
        result = await self._send_command("maximize_window", {"window_id": window_id})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to maximize window"))

    async def minimize_window(self, window_id: int | str) -> None:
        result = await self._send_command("minimize_window", {"window_id": window_id})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to minimize window"))

    async def activate_window(self, window_id: int | str) -> None:
        result = await self._send_command("activate_window", {"window_id": window_id})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to activate window"))

    async def close_window(self, window_id: int | str) -> None:
        result = await self._send_command("close_window", {"window_id": window_id})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to close window"))

    # Convenience aliases
    async def get_window_title(self, window_id: int | str) -> str:
        return await self.get_window_name(window_id)

    async def window_size(self, window_id: int | str) -> tuple[int, int]:
        return await self.get_window_size(window_id)

    # Command execution
    async def run_command(self, command: str) -> CommandResult:
        result = await self._send_command("run_command", {"command": command})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to run command"))
        return CommandResult(
            stdout=result.get("stdout", ""),
            stderr=result.get("stderr", ""),
            returncode=result.get("return_code", 0),
        )

    # Accessibility Actions
    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree of the current screen."""
        result = await self._send_command("get_accessibility_tree")
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get accessibility tree"))
        return result

    async def get_active_window_bounds(self) -> Dict[str, int]:
        """Get the bounds of the currently active window."""
        result = await self._send_command("get_active_window_bounds")
        if result["success"] and result["bounds"]:
            return result["bounds"]
        raise RuntimeError("Failed to get active window bounds")

    async def to_screen_coordinates(self, x: float, y: float) -> tuple[float, float]:
        """Convert screenshot coordinates to screen coordinates.

        Args:
            x: X coordinate in screenshot space
            y: Y coordinate in screenshot space

        Returns:
            tuple[float, float]: (x, y) coordinates in screen space
        """
        screen_size = await self.get_screen_size()
        screenshot = await self.screenshot()
        screenshot_img = bytes_to_image(screenshot)
        screenshot_width, screenshot_height = screenshot_img.size

        # Calculate scaling factors
        width_scale = screen_size["width"] / screenshot_width
        height_scale = screen_size["height"] / screenshot_height

        # Convert coordinates
        screen_x = x * width_scale
        screen_y = y * height_scale

        return screen_x, screen_y

    async def to_screenshot_coordinates(self, x: float, y: float) -> tuple[float, float]:
        """Convert screen coordinates to screenshot coordinates.

        Args:
            x: X coordinate in screen space
            y: Y coordinate in screen space

        Returns:
            tuple[float, float]: (x, y) coordinates in screenshot space
        """
        screen_size = await self.get_screen_size()
        screenshot = await self.screenshot()
        screenshot_img = bytes_to_image(screenshot)
        screenshot_width, screenshot_height = screenshot_img.size

        # Calculate scaling factors
        width_scale = screenshot_width / screen_size["width"]
        height_scale = screenshot_height / screen_size["height"]

        # Convert coordinates
        screenshot_x = x * width_scale
        screenshot_y = y * height_scale

        return screenshot_x, screenshot_y

    # Playwright browser control
    async def playwright_exec(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        """
        Execute a Playwright browser command.

        Args:
            command: The browser command to execute (visit_url, click, type, scroll, web_search)
            params: Command parameters

        Returns:
            Dict containing the command result

        Examples:
            # Navigate to a URL
            await interface.playwright_exec("visit_url", {"url": "https://example.com"})

            # Click at coordinates
            await interface.playwright_exec("click", {"x": 100, "y": 200})

            # Type text
            await interface.playwright_exec("type", {"text": "Hello, world!"})

            # Scroll
            await interface.playwright_exec("scroll", {"delta_x": 0, "delta_y": -100})

            # Web search
            await interface.playwright_exec("web_search", {"query": "computer use agent"})
        """
        protocol = "https" if self.api_key else "http"
        port = "8443" if self.api_key else "8000"
        url = f"{protocol}://{self.ip_address}:{port}/playwright_exec"

        payload = {"command": command, "params": params or {}}
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["X-API-Key"] = self.api_key
        if self.vm_name:
            headers["X-Container-Name"] = self.vm_name

        try:
            async with aiohttp.ClientSession() as session:
                async with session.post(url, json=payload, headers=headers) as response:
                    if response.status == 200:
                        return await response.json()
                    else:
                        error_text = await response.text()
                        return {"success": False, "error": error_text}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Websocket Methods
    async def _keep_alive(self):
        """Keep the WebSocket connection alive with automatic reconnection."""
        retry_count = 0
        max_log_attempts = 1  # Only log the first attempt at INFO level
        log_interval = 500  # Then log every 500th attempt (significantly increased from 30)
        last_warning_time = 0
        min_warning_interval = 30  # Minimum seconds between connection lost warnings
        min_retry_delay = 0.5  # Minimum delay between connection attempts (500ms)

        while not self._closed:
            try:
                if self._ws is None or (
                    self._ws and self._ws.state == websockets.protocol.State.CLOSED
                ):
                    try:
                        retry_count += 1

                        # Add a minimum delay between connection attempts to avoid flooding
                        if retry_count > 1:
                            await asyncio.sleep(min_retry_delay)

                        # Only log the first attempt at INFO level, then every Nth attempt
                        if retry_count == 1:
                            self.logger.info(f"Attempting WebSocket connection to {self.ws_uri}")
                        elif retry_count % log_interval == 0:
                            self.logger.info(
                                f"Still attempting WebSocket connection (attempt {retry_count})..."
                            )
                        else:
                            # All other attempts are logged at DEBUG level
                            self.logger.debug(
                                f"Attempting WebSocket connection to {self.ws_uri} (attempt {retry_count})"
                            )

                        self._ws = await asyncio.wait_for(
                            websockets.connect(
                                self.ws_uri,
                                max_size=1024 * 1024 * 10,  # 10MB limit
                                max_queue=32,
                                ping_interval=self._ping_interval,
                                ping_timeout=self._ping_timeout,
                                close_timeout=5,
                                compression=None,  # Disable compression to reduce overhead
                            ),
                            timeout=120,
                        )
                        self.logger.info("WebSocket connection established")

                        # If api_key and vm_name are provided, perform authentication handshake
                        if self.api_key and self.vm_name:
                            self.logger.info("Performing authentication handshake...")
                            auth_message = {
                                "command": "authenticate",
                                "params": {"api_key": self.api_key, "container_name": self.vm_name},
                            }
                            await self._ws.send(json.dumps(auth_message))

                            # Wait for authentication response
                            async with self._recv_lock:
                                auth_response = await asyncio.wait_for(self._ws.recv(), timeout=10)
                            auth_result = json.loads(auth_response)

                            if not auth_result.get("success"):
                                error_msg = auth_result.get("error", "Authentication failed")
                                self.logger.error(f"Authentication failed: {error_msg}")
                                await self._ws.close()
                                self._ws = None
                                raise ConnectionError(f"Authentication failed: {error_msg}")

                            self.logger.info("Authentication successful")

                        self._reconnect_delay = 1  # Reset reconnect delay on successful connection
                        self._last_ping = time.time()
                        retry_count = 0  # Reset retry count on successful connection
                    except (asyncio.TimeoutError, websockets.exceptions.WebSocketException) as e:
                        next_retry = self._reconnect_delay

                        # Only log the first error at WARNING level, then every Nth attempt
                        if retry_count == 1:
                            self.logger.warning(
                                "Computer API Server not ready yet. Will retry automatically."
                            )
                        elif retry_count % log_interval == 0:
                            self.logger.warning(
                                f"Still waiting for Computer API Server (attempt {retry_count})..."
                            )
                        else:
                            # All other errors are logged at DEBUG level
                            self.logger.debug(f"Connection attempt {retry_count} failed: {e}")

                        if self._ws:
                            try:
                                await self._ws.close()
                            except:
                                pass
                        self._ws = None

                        # Use exponential backoff for connection retries
                        await asyncio.sleep(self._reconnect_delay)
                        self._reconnect_delay = min(
                            self._reconnect_delay * 2, self._max_reconnect_delay
                        )
                        continue

                # Regular ping to check connection
                if self._ws and self._ws.state == websockets.protocol.State.OPEN:
                    try:
                        if time.time() - self._last_ping >= self._ping_interval:
                            pong_waiter = await self._ws.ping()
                            await asyncio.wait_for(pong_waiter, timeout=self._ping_timeout)
                            self._last_ping = time.time()
                    except Exception as e:
                        self.logger.debug(f"Ping failed: {e}")
                        if self._ws:
                            try:
                                await self._ws.close()
                            except:
                                pass
                        self._ws = None
                        continue

                await asyncio.sleep(1)

            except Exception as e:
                current_time = time.time()
                # Only log connection lost warnings at most once every min_warning_interval seconds
                if current_time - last_warning_time >= min_warning_interval:
                    self.logger.warning(
                        f"Computer API Server connection lost while attempting to connect to "
                        f"{self.ip_address or 'localhost'}. "
                        "The server may not be running or is not reachable yet. Retrying..."
                    )
                    last_warning_time = current_time
                else:
                    # Log at debug level instead
                    self.logger.debug(f"Connection lost: {e}")

                if self._ws:
                    try:
                        await self._ws.close()
                    except:
                        pass
                self._ws = None

    async def _ensure_connection(self):
        """Ensure WebSocket connection is established."""
        if self._reconnect_task is None or self._reconnect_task.done():
            self._reconnect_task = asyncio.create_task(self._keep_alive())

        retry_count = 0
        max_retries = 5

        while retry_count < max_retries:
            try:
                if self._ws and self._ws.state == websockets.protocol.State.OPEN:
                    return
                retry_count += 1
                await asyncio.sleep(1)
            except Exception as e:
                # Only log at ERROR level for the last retry attempt
                if retry_count == max_retries - 1:
                    self.logger.error(
                        f"Persistent connection check error after {retry_count} attempts: {e}"
                    )
                else:
                    self.logger.debug(f"Connection check error (attempt {retry_count}): {e}")
                retry_count += 1
                await asyncio.sleep(1)
                continue

        raise ConnectionError("Failed to establish WebSocket connection after multiple retries")

    async def _send_command_ws(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        """Send command through WebSocket."""
        max_retries = 3
        retry_count = 0
        last_error = None

        # Acquire lock to ensure only one command is processed at a time
        self.logger.debug(f"Acquired lock for command: {command}")
        while retry_count < max_retries:
            try:
                await self._ensure_connection()
                if not self._ws:
                    raise ConnectionError("WebSocket connection is not established")

                message = {"command": command, "params": params or {}}
                await self._ws.send(json.dumps(message))
                async with self._recv_lock:
                    response = await asyncio.wait_for(self._ws.recv(), timeout=120)
                self.logger.debug(f"Completed command: {command}")
                return json.loads(response)
            except Exception as e:
                last_error = e
                retry_count += 1
                if retry_count < max_retries:
                    # Only log at debug level for intermediate retries
                    self.logger.debug(
                        f"Command '{command}' failed (attempt {retry_count}/{max_retries}): {e}"
                    )
                    await asyncio.sleep(1)
                    continue
                else:
                    # Only log at error level for the final failure
                    self.logger.error(
                        f"Failed to send command '{command}' after {max_retries} retries"
                    )
                    self.logger.debug(f"Command failure details: {e}")
                    raise

        raise last_error if last_error else RuntimeError("Failed to send command")

    async def _send_command_rest(
        self, command: str, params: Optional[Dict] = None
    ) -> Dict[str, Any]:
        """Send command through REST API without retries or connection management."""
        try:
            # Prepare the request payload
            payload = {"command": command, "params": params or {}}

            # Prepare headers
            headers = {"Content-Type": "application/json"}
            if self.api_key:
                headers["X-API-Key"] = self.api_key
            if self.vm_name:
                headers["X-Container-Name"] = self.vm_name

            # Send the request
            async with aiohttp.ClientSession() as session:
                async with session.post(self.rest_uri, json=payload, headers=headers) as response:
                    # Get the response text
                    response_text = await response.text()

                    # Trim whitespace
                    response_text = response_text.strip()

                    # Check if it starts with "data: "
                    if response_text.startswith("data: "):
                        # Extract everything after "data: "
                        json_str = response_text[6:]  # Remove "data: " prefix
                        try:
                            return json.loads(json_str)
                        except json.JSONDecodeError:
                            return {
                                "success": False,
                                "error": "Server returned malformed response",
                                "message": response_text,
                            }
                    else:
                        # Return error response
                        return {
                            "success": False,
                            "error": "Server returned malformed response",
                            "message": response_text,
                        }

        except Exception as e:
            return {"success": False, "error": "Request failed", "message": str(e)}

    async def _send_command(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        """Send command using REST API with WebSocket fallback."""
        # Try REST API first
        result = await self._send_command_rest(command, params)

        # If REST failed with "Request failed", try WebSocket as fallback
        if not result.get("success", True) and (
            result.get("error") == "Request failed"
            or result.get("error") == "Server returned malformed response"
        ):
            self.logger.warning(
                f"REST API failed for command '{command}', trying WebSocket fallback"
            )
            try:
                return await self._send_command_ws(command, params)
            except Exception as e:
                self.logger.error(f"WebSocket fallback also failed: {e}")
                # Return the original REST error
                return result

        return result

    async def wait_for_ready(self, timeout: int = 60, interval: float = 1.0):
        """Wait for Computer API Server to be ready by testing version command."""

        # Check if REST API is available
        try:
            result = await self._send_command_rest("version", {})
            assert result.get("success", True)
        except Exception as e:
            self.logger.debug(
                f"REST API failed for command 'version', trying WebSocket fallback: {e}"
            )
            try:
                await self._wait_for_ready_ws(timeout, interval)
                return
            except Exception as e:
                self.logger.debug(f"WebSocket fallback also failed: {e}")
                raise e

        start_time = time.time()
        last_error = None
        attempt_count = 0
        progress_interval = 10  # Log progress every 10 seconds
        last_progress_time = start_time

        try:
            self.logger.info(
                f"Waiting for Computer API Server to be ready (timeout: {timeout}s)..."
            )

            # Wait for the server to respond to get_screen_size command
            while time.time() - start_time < timeout:
                try:
                    attempt_count += 1
                    current_time = time.time()

                    # Log progress periodically without flooding logs
                    if current_time - last_progress_time >= progress_interval:
                        elapsed = current_time - start_time
                        self.logger.info(
                            f"Still waiting for Computer API Server... (elapsed: {elapsed:.1f}s, attempts: {attempt_count})"
                        )
                        last_progress_time = current_time

                    # Test the server with a simple get_screen_size command
                    result = await self._send_command("get_screen_size")
                    if result.get("success", False):
                        elapsed = time.time() - start_time
                        self.logger.info(
                            f"Computer API Server is ready (after {elapsed:.1f}s, {attempt_count} attempts)"
                        )
                        return  # Server is ready
                    else:
                        last_error = result.get("error", "Unknown error")
                        self.logger.debug(f"Initial connection command failed: {last_error}")

                except Exception as e:
                    last_error = e
                    self.logger.debug(f"Connection attempt {attempt_count} failed: {e}")

                # Wait before trying again
                await asyncio.sleep(interval)

            # If we get here, we've timed out
            error_msg = f"Could not connect to {self.ip_address} after {timeout} seconds"
            if last_error:
                error_msg += f": {str(last_error)}"
            self.logger.error(error_msg)
            raise TimeoutError(error_msg)

        except Exception as e:
            if isinstance(e, TimeoutError):
                raise

            target = f"{self.ip_address}:{self._api_port}" if self._api_port else self.ip_address
            
            error_msg = (
                f"Failed to connect to Computer API Server at {target}. "
                f"Reason: {str(e)}. "
                "Ensure the server is running and reachable. "
                "If using Lume or Docker, verify the VM has started and the API port is open."
            )
            self.logger.error(error_msg)
            raise RuntimeError(error_msg)

    async def _wait_for_ready_ws(self, timeout: int = 60, interval: float = 1.0):
        """Wait for WebSocket connection to become available."""
        start_time = time.time()
        last_error = None
        attempt_count = 0
        progress_interval = 10  # Log progress every 10 seconds
        last_progress_time = start_time

        # Disable detailed logging for connection attempts
        self._log_connection_attempts = False

        try:
            self.logger.info(
                f"Waiting for Computer API Server to be ready (timeout: {timeout}s)..."
            )

            # Start the keep-alive task if it's not already running
            if self._reconnect_task is None or self._reconnect_task.done():
                self._reconnect_task = asyncio.create_task(self._keep_alive())

            # Wait for the connection to be established
            while time.time() - start_time < timeout:
                try:
                    attempt_count += 1
                    current_time = time.time()

                    # Log progress periodically without flooding logs
                    if current_time - last_progress_time >= progress_interval:
                        elapsed = current_time - start_time
                        self.logger.info(
                            f"Still waiting for Computer API Server... (elapsed: {elapsed:.1f}s, attempts: {attempt_count})"
                        )
                        last_progress_time = current_time

                    # Check if we have a connection
                    if self._ws and self._ws.state == websockets.protocol.State.OPEN:
                        # Test the connection with a simple command
                        try:
                            await self._send_command_ws("get_screen_size")
                            elapsed = time.time() - start_time
                            self.logger.info(
                                f"Computer API Server is ready (after {elapsed:.1f}s, {attempt_count} attempts)"
                            )
                            return  # Connection is fully working
                        except Exception as e:
                            last_error = e
                            self.logger.debug(f"Connection test failed: {e}")

                    # Wait before trying again
                    await asyncio.sleep(interval)

                except Exception as e:
                    last_error = e
                    self.logger.debug(f"Connection attempt {attempt_count} failed: {e}")
                    await asyncio.sleep(interval)

            # If we get here, we've timed out
            error_msg = f"Could not connect to {self.ip_address} after {timeout} seconds"
            if last_error:
                error_msg += f": {str(last_error)}"
            self.logger.error(error_msg)
            raise TimeoutError(error_msg)
        finally:
            # Reset to default logging behavior
            self._log_connection_attempts = False

    def close(self):
        """Close WebSocket connection.

        Note: In host computer server mode, we leave the connection open
        to allow other clients to connect to the same server. The server
        will handle cleaning up idle connections.
        """
        # Only cancel the reconnect task
        if self._reconnect_task:
            self._reconnect_task.cancel()

        # Don't set closed flag or close websocket by default
        # This allows the server to stay connected for other clients
        # self._closed = True
        # if self._ws:
        #     asyncio.create_task(self._ws.close())
        #     self._ws = None

    def force_close(self):
        """Force close the WebSocket connection.

        This method should be called when you want to completely
        shut down the connection, not just for regular cleanup.
        """
        self._closed = True
        if self._reconnect_task:
            self._reconnect_task.cancel()
        if self._ws:
            asyncio.create_task(self._ws.close())
            self._ws = None
