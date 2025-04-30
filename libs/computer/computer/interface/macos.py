import asyncio
import json
import time
from typing import Any, Dict, List, Optional, Tuple
from PIL import Image

import websockets

from ..logger import Logger, LogLevel
from .base import BaseComputerInterface
from ..utils import decode_base64_image, bytes_to_image, draw_box, resize_image
from .models import Key, KeyType


class MacOSComputerInterface(BaseComputerInterface):
    """Interface for MacOS."""

    def __init__(self, ip_address: str, username: str = "lume", password: str = "lume"):
        super().__init__(ip_address, username, password)
        self.ws_uri = f"ws://{ip_address}:8000/ws"
        self._ws = None
        self._reconnect_task = None
        self._closed = False
        self._last_ping = 0
        self._ping_interval = 5  # Send ping every 5 seconds
        self._ping_timeout = 10  # Wait 10 seconds for pong response
        self._reconnect_delay = 1  # Start with 1 second delay
        self._max_reconnect_delay = 30  # Maximum delay between reconnection attempts
        self._log_connection_attempts = True  # Flag to control connection attempt logging

        # Set logger name for MacOS interface
        self.logger = Logger("cua.interface.macos", LogLevel.NORMAL)

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
                            timeout=30,
                        )
                        self.logger.info("WebSocket connection established")
                        self._reconnect_delay = 1  # Reset reconnect delay on successful connection
                        self._last_ping = time.time()
                        retry_count = 0  # Reset retry count on successful connection
                    except (asyncio.TimeoutError, websockets.exceptions.WebSocketException) as e:
                        next_retry = self._reconnect_delay

                        # Only log the first error at WARNING level, then every Nth attempt
                        if retry_count == 1:
                            self.logger.warning(
                                f"Computer API Server not ready yet. Will retry automatically."
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
                        f"Computer API Server connection lost. Will retry automatically."
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

    async def _send_command(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        """Send command through WebSocket."""
        max_retries = 3
        retry_count = 0
        last_error = None

        while retry_count < max_retries:
            try:
                await self._ensure_connection()
                if not self._ws:
                    raise ConnectionError("WebSocket connection is not established")

                message = {"command": command, "params": params or {}}
                await self._ws.send(json.dumps(message))
                response = await asyncio.wait_for(self._ws.recv(), timeout=30)
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

    async def wait_for_ready(self, timeout: int = 60, interval: float = 1.0):
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
                            await self._send_command("get_screen_size")
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

    # Mouse Actions
    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        await self._send_command("left_click", {"x": x, "y": y})

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        await self._send_command("right_click", {"x": x, "y": y})

    async def double_click(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        await self._send_command("double_click", {"x": x, "y": y})

    async def move_cursor(self, x: int, y: int) -> None:
        await self._send_command("move_cursor", {"x": x, "y": y})

    async def drag_to(self, x: int, y: int, button: str = "left", duration: float = 0.5) -> None:
        await self._send_command(
            "drag_to", {"x": x, "y": y, "button": button, "duration": duration}
        )

    async def drag(self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5) -> None:
        await self._send_command(
            "drag", {"path": path, "button": button, "duration": duration}
        )

    # Keyboard Actions
    async def type_text(self, text: str) -> None:
        await self._send_command("type_text", {"text": text})

    async def press(self, key: "KeyType") -> None:
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

    async def press_key(self, key: "KeyType") -> None:
        """DEPRECATED: Use press() instead.

        This method is kept for backward compatibility but will be removed in a future version.
        Please use the press() method instead.
        """
        await self.press(key)

    async def hotkey(self, *keys: "KeyType") -> None:
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
                actual_keys.append(key_or_enum.value if isinstance(key_or_enum, Key) else key_or_enum)
            else:
                raise ValueError(f"Invalid key type: {type(key)}. Must be Key enum or string.")
        
        await self._send_command("hotkey", {"keys": actual_keys})

    # Scrolling Actions
    async def scroll_down(self, clicks: int = 1) -> None:
        await self._send_command("scroll_down", {"clicks": clicks})
        
    async def scroll_up(self, clicks: int = 1) -> None:
        await self._send_command("scroll_up", {"clicks": clicks})

    # Screen Actions
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
            raise RuntimeError("Failed to take screenshot")

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

    # File System Actions
    async def file_exists(self, path: str) -> bool:
        result = await self._send_command("file_exists", {"path": path})
        return result.get("exists", False)

    async def directory_exists(self, path: str) -> bool:
        result = await self._send_command("directory_exists", {"path": path})
        return result.get("exists", False)

    async def run_command(self, command: str) -> Tuple[str, str]:
        result = await self._send_command("run_command", {"command": command})
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to run command"))
        return result.get("stdout", ""), result.get("stderr", "")

    # Accessibility Actions
    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree of the current screen."""
        result = await self._send_command("get_accessibility_tree")
        if not result.get("success", False):
            raise RuntimeError(result.get("error", "Failed to get accessibility tree"))
        return result.get("tree", {})

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
