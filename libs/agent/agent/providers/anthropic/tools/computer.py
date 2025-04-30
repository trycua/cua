import asyncio
import base64
import io
import logging
from enum import StrEnum
from pathlib import Path
from typing import Literal, TypedDict, Any, Dict
import subprocess
from PIL import Image
from datetime import datetime

from computer.computer import Computer

from .base import BaseAnthropicTool, ToolError, ToolResult
from .run import run
from ....core.tools.computer import BaseComputerTool

TYPING_DELAY_MS = 12
TYPING_GROUP_SIZE = 50

Action = Literal[
    "key",
    "type",
    "mouse_move",
    "left_click",
    "left_click_drag",
    "right_click",
    "middle_click",
    "double_click",
    "screenshot",
    "cursor_position",
    "scroll",
]


class Resolution(TypedDict):
    width: int
    height: int


class ScalingSource(StrEnum):
    COMPUTER = "computer"
    API = "api"


class ComputerToolOptions(TypedDict):
    display_height_px: int
    display_width_px: int
    display_number: int | None


def chunks(s: str, chunk_size: int) -> list[str]:
    return [s[i : i + chunk_size] for i in range(0, len(s), chunk_size)]


class ComputerTool(BaseComputerTool, BaseAnthropicTool):
    """
    A tool that allows the agent to interact with the screen, keyboard, and mouse of the current macOS computer.
    The tool parameters are defined by Anthropic and are not editable.
    """

    name: Literal["computer"] = "computer"
    api_type: Literal["computer_20250124"] = "computer_20250124"
    width: int | None = None
    height: int | None = None
    display_num: int | None = None
    computer: Computer  # The CUA Computer instance
    logger = logging.getLogger(__name__)

    _screenshot_delay = 1.0  # macOS is generally faster than X11
    _scaling_enabled = True

    @property
    def options(self) -> ComputerToolOptions:
        if self.width is None or self.height is None:
            raise RuntimeError(
                "Screen dimensions not initialized. Call initialize_dimensions() first."
            )
        return {
            "display_width_px": self.width,
            "display_height_px": self.height,
            "display_number": self.display_num,
        }

    def to_params(self) -> Dict[str, Any]:
        """Convert tool to API parameters.

        Returns:
            Dictionary with tool parameters
        """
        return {"name": self.name, "type": self.api_type, **self.options}

    def __init__(self, computer):
        # Initialize the base computer tool first
        BaseComputerTool.__init__(self, computer)
        # Then initialize the Anthropic tool
        BaseAnthropicTool.__init__(self)

        # Additional initialization
        self.width = None  # Will be initialized from computer interface
        self.height = None  # Will be initialized from computer interface
        self.display_num = None

    async def initialize_dimensions(self):
        """Initialize screen dimensions from the computer interface."""
        display_size = await self.computer.interface.get_screen_size()
        self.width = display_size["width"]
        self.height = display_size["height"]
        assert isinstance(self.width, int) and isinstance(self.height, int)
        self.logger.info(f"Initialized screen dimensions to {self.width}x{self.height}")

    async def __call__(
        self,
        *,
        action: Action,
        text: str | None = None,
        coordinate: tuple[int, int] | None = None,
        **kwargs,
    ):
        try:
            # Ensure dimensions are initialized
            if self.width is None or self.height is None:
                await self.initialize_dimensions()
                if self.width is None or self.height is None:
                    raise ToolError("Failed to initialize screen dimensions")
        except Exception as e:
            raise ToolError(f"Failed to initialize dimensions: {e}")

        if action in ("mouse_move", "left_click_drag"):
            if coordinate is None:
                raise ToolError(f"coordinate is required for {action}")
            if text is not None:
                raise ToolError(f"text is not accepted for {action}")
            if not isinstance(coordinate, (list, tuple)) or len(coordinate) != 2:
                raise ToolError(f"{coordinate} must be a tuple of length 2")
            if not all(isinstance(i, int) and i >= 0 for i in coordinate):
                raise ToolError(f"{coordinate} must be a tuple of non-negative ints")

            try:
                x, y = coordinate
                self.logger.info(f"Handling {action} action:")
                self.logger.info(f"  Coordinates: ({x}, {y})")

                # Take pre-action screenshot to get current dimensions
                pre_screenshot = await self.computer.interface.screenshot()
                pre_img = Image.open(io.BytesIO(pre_screenshot))

                # Scale image to match screen dimensions if needed
                if pre_img.size != (self.width, self.height):
                    self.logger.info(
                        f"Scaling image from {pre_img.size} to {self.width}x{self.height} to match screen dimensions"
                    )
                    if not isinstance(self.width, int) or not isinstance(self.height, int):
                        raise ToolError("Screen dimensions must be integers")
                    size = (int(self.width), int(self.height))
                    pre_img = pre_img.resize(size, Image.Resampling.LANCZOS)

                self.logger.info(f"  Current dimensions: {pre_img.width}x{pre_img.height}")

                if action == "mouse_move":
                    self.logger.info(f"Moving cursor to ({x}, {y})")
                    await self.computer.interface.move_cursor(x, y)
                elif action == "left_click_drag":
                    # Get the start coordinate from kwargs
                    start_coordinate = kwargs.get("start_coordinate")
                    if not start_coordinate:
                        raise ToolError("start_coordinate is required for left_click_drag action")
                    
                    start_x, start_y = start_coordinate
                    end_x, end_y = x, y
                    
                    self.logger.info(f"Dragging from ({start_x}, {start_y}) to ({end_x}, {end_y})")
                    await self.computer.interface.move_cursor(start_x, start_y)
                    await self.computer.interface.drag_to(end_x, end_y)

                # Wait briefly for any UI changes
                await asyncio.sleep(0.5)

                # Take post-action screenshot
                post_screenshot = await self.computer.interface.screenshot()
                post_img = Image.open(io.BytesIO(post_screenshot))

                # Scale post-action image if needed
                if post_img.size != (self.width, self.height):
                    self.logger.info(
                        f"Scaling post-action image from {post_img.size} to {self.width}x{self.height}"
                    )
                    post_img = post_img.resize((self.width, self.height), Image.Resampling.LANCZOS)
                    buffer = io.BytesIO()
                    post_img.save(buffer, format="PNG")
                    post_screenshot = buffer.getvalue()

                return ToolResult(
                    output=f"{'Moved cursor to' if action == 'mouse_move' else 'Dragged to'} {x},{y}",
                    base64_image=base64.b64encode(post_screenshot).decode(),
                )
            except Exception as e:
                self.logger.error(f"Error during {action} action: {str(e)}")
                raise ToolError(f"Failed to perform {action}: {str(e)}")

        elif action in ("left_click", "right_click", "double_click"):
            if coordinate:
                x, y = coordinate
                self.logger.info(f"Handling {action} action:")
                self.logger.info(f"  Coordinates: ({x}, {y})")

                try:
                    # Take pre-action screenshot to get current dimensions
                    pre_screenshot = await self.computer.interface.screenshot()
                    pre_img = Image.open(io.BytesIO(pre_screenshot))

                    # Scale image to match screen dimensions if needed
                    if pre_img.size != (self.width, self.height):
                        self.logger.info(
                            f"Scaling image from {pre_img.size} to {self.width}x{self.height} to match screen dimensions"
                        )
                        if not isinstance(self.width, int) or not isinstance(self.height, int):
                            raise ToolError("Screen dimensions must be integers")
                        size = (int(self.width), int(self.height))
                        pre_img = pre_img.resize(size, Image.Resampling.LANCZOS)
                        # Save the scaled image back to bytes
                        buffer = io.BytesIO()
                        pre_img.save(buffer, format="PNG")
                        pre_screenshot = buffer.getvalue()

                    self.logger.info(f"  Current dimensions: {pre_img.width}x{pre_img.height}")

                    # Perform the click action
                    if action == "left_click":
                        self.logger.info(f"Clicking at ({x}, {y})")
                        await self.computer.interface.move_cursor(x, y)
                        await self.computer.interface.left_click()
                    elif action == "right_click":
                        self.logger.info(f"Right clicking at ({x}, {y})")
                        await self.computer.interface.move_cursor(x, y)
                        await self.computer.interface.right_click()
                    elif action == "double_click":
                        self.logger.info(f"Double clicking at ({x}, {y})")
                        await self.computer.interface.move_cursor(x, y)
                        await self.computer.interface.double_click()

                    # Wait briefly for any UI changes
                    await asyncio.sleep(0.5)

                    # Take and save post-action screenshot
                    post_screenshot = await self.computer.interface.screenshot()
                    post_img = Image.open(io.BytesIO(post_screenshot))

                    # Scale post-action image if needed
                    if post_img.size != (self.width, self.height):
                        self.logger.info(
                            f"Scaling post-action image from {post_img.size} to {self.width}x{self.height}"
                        )
                        post_img = post_img.resize(
                            (self.width, self.height), Image.Resampling.LANCZOS
                        )
                        buffer = io.BytesIO()
                        post_img.save(buffer, format="PNG")
                        post_screenshot = buffer.getvalue()

                    return ToolResult(
                        output=f"Performed {action} at ({x}, {y})",
                        base64_image=base64.b64encode(post_screenshot).decode(),
                    )
                except Exception as e:
                    self.logger.error(f"Error during {action} action: {str(e)}")
                    raise ToolError(f"Failed to perform {action}: {str(e)}")
            else:
                try:
                    # Take pre-action screenshot
                    pre_screenshot = await self.computer.interface.screenshot()
                    pre_img = Image.open(io.BytesIO(pre_screenshot))

                    # Scale image if needed
                    if pre_img.size != (self.width, self.height):
                        self.logger.info(
                            f"Scaling image from {pre_img.size} to {self.width}x{self.height}"
                        )
                        if not isinstance(self.width, int) or not isinstance(self.height, int):
                            raise ToolError("Screen dimensions must be integers")
                        size = (int(self.width), int(self.height))
                        pre_img = pre_img.resize(size, Image.Resampling.LANCZOS)

                    # Perform the click action
                    if action == "left_click":
                        self.logger.info("Performing left click at current position")
                        await self.computer.interface.left_click()
                    elif action == "right_click":
                        self.logger.info("Performing right click at current position")
                        await self.computer.interface.right_click()
                    elif action == "double_click":
                        self.logger.info("Performing double click at current position")
                        await self.computer.interface.double_click()

                    # Wait briefly for any UI changes
                    await asyncio.sleep(0.5)

                    # Take post-action screenshot
                    post_screenshot = await self.computer.interface.screenshot()
                    post_img = Image.open(io.BytesIO(post_screenshot))

                    # Scale post-action image if needed
                    if post_img.size != (self.width, self.height):
                        self.logger.info(
                            f"Scaling post-action image from {post_img.size} to {self.width}x{self.height}"
                        )
                        post_img = post_img.resize(
                            (self.width, self.height), Image.Resampling.LANCZOS
                        )
                        buffer = io.BytesIO()
                        post_img.save(buffer, format="PNG")
                        post_screenshot = buffer.getvalue()

                    return ToolResult(
                        output=f"Performed {action} at current position",
                        base64_image=base64.b64encode(post_screenshot).decode(),
                    )
                except Exception as e:
                    self.logger.error(f"Error during {action} action: {str(e)}")
                    raise ToolError(f"Failed to perform {action}: {str(e)}")

        elif action in ("key", "type"):
            if text is None:
                raise ToolError(f"text is required for {action}")
            if coordinate is not None:
                raise ToolError(f"coordinate is not accepted for {action}")
            if not isinstance(text, str):
                raise ToolError(f"{text} must be a string")

            try:
                # Take pre-action screenshot
                pre_screenshot = await self.computer.interface.screenshot()
                pre_img = Image.open(io.BytesIO(pre_screenshot))

                # Scale image if needed
                if pre_img.size != (self.width, self.height):
                    self.logger.info(
                        f"Scaling image from {pre_img.size} to {self.width}x{self.height}"
                    )
                    if not isinstance(self.width, int) or not isinstance(self.height, int):
                        raise ToolError("Screen dimensions must be integers")
                    size = (int(self.width), int(self.height))
                    pre_img = pre_img.resize(size, Image.Resampling.LANCZOS)

                if action == "key":
                    # Special handling for page up/down on macOS
                    if text.lower() in ["pagedown", "page_down", "page down"]:
                        self.logger.info("Converting page down to fn+down for macOS")
                        await self.computer.interface.hotkey("fn", "down")
                        output_text = "fn+down"
                    elif text.lower() in ["pageup", "page_up", "page up"]:
                        self.logger.info("Converting page up to fn+up for macOS")
                        await self.computer.interface.hotkey("fn", "up")
                        output_text = "fn+up"
                    elif text == "fn+down":
                        self.logger.info("Using fn+down combination")
                        await self.computer.interface.hotkey("fn", "down")
                        output_text = text
                    elif text == "fn+up":
                        self.logger.info("Using fn+up combination")
                        await self.computer.interface.hotkey("fn", "up")
                        output_text = text
                    elif "+" in text:
                        # Handle hotkey combinations
                        keys = text.split("+")
                        self.logger.info(f"Pressing hotkey combination: {text}")
                        await self.computer.interface.hotkey(*keys)
                        output_text = text
                    else:
                        # Handle single key press
                        self.logger.info(f"Pressing key: {text}")
                        try:
                            await self.computer.interface.press_key(text)
                            output_text = text
                        except ValueError as e:
                            raise ToolError(f"Invalid key: {text}. {str(e)}")

                    # Wait briefly for UI changes
                    await asyncio.sleep(0.5)

                    # Take post-action screenshot
                    post_screenshot = await self.computer.interface.screenshot()
                    post_img = Image.open(io.BytesIO(post_screenshot))

                    # Scale post-action image if needed
                    if post_img.size != (self.width, self.height):
                        self.logger.info(
                            f"Scaling post-action image from {post_img.size} to {self.width}x{self.height}"
                        )
                        post_img = post_img.resize(
                            (self.width, self.height), Image.Resampling.LANCZOS
                        )
                        buffer = io.BytesIO()
                        post_img.save(buffer, format="PNG")
                        post_screenshot = buffer.getvalue()

                    return ToolResult(
                        output=f"Pressed key: {output_text}",
                        base64_image=base64.b64encode(post_screenshot).decode(),
                    )

                elif action == "type":
                    self.logger.info(f"Typing text: {text}")
                    await self.computer.interface.type_text(text)

                    # Wait briefly for UI changes
                    await asyncio.sleep(0.5)

                    # Take post-action screenshot
                    post_screenshot = await self.computer.interface.screenshot()
                    post_img = Image.open(io.BytesIO(post_screenshot))

                    # Scale post-action image if needed
                    if post_img.size != (self.width, self.height):
                        self.logger.info(
                            f"Scaling post-action image from {post_img.size} to {self.width}x{self.height}"
                        )
                        post_img = post_img.resize(
                            (self.width, self.height), Image.Resampling.LANCZOS
                        )
                        buffer = io.BytesIO()
                        post_img.save(buffer, format="PNG")
                        post_screenshot = buffer.getvalue()

                    return ToolResult(
                        output=f"Typed text: {text}",
                        base64_image=base64.b64encode(post_screenshot).decode(),
                    )
            except Exception as e:
                self.logger.error(f"Error during {action} action: {str(e)}")
                raise ToolError(f"Failed to perform {action}: {str(e)}")

        elif action in ("screenshot", "cursor_position"):
            if text is not None:
                raise ToolError(f"text is not accepted for {action}")
            if coordinate is not None:
                raise ToolError(f"coordinate is not accepted for {action}")

            try:
                if action == "screenshot":
                    # Take screenshot
                    screenshot = await self.computer.interface.screenshot()
                    img = Image.open(io.BytesIO(screenshot))

                    # Scale image if needed
                    if img.size != (self.width, self.height):
                        self.logger.info(
                            f"Scaling image from {img.size} to {self.width}x{self.height}"
                        )
                        if not isinstance(self.width, int) or not isinstance(self.height, int):
                            raise ToolError("Screen dimensions must be integers")
                        size = (int(self.width), int(self.height))
                        img = img.resize(size, Image.Resampling.LANCZOS)
                        buffer = io.BytesIO()
                        img.save(buffer, format="PNG")
                        screenshot = buffer.getvalue()

                    return ToolResult(base64_image=base64.b64encode(screenshot).decode())

                elif action == "cursor_position":
                    pos = await self.computer.interface.get_cursor_position()
                    x, y = pos  # Unpack the tuple
                    return ToolResult(output=f"X={int(x)},Y={int(y)}")

            except Exception as e:
                self.logger.error(f"Error during {action} action: {str(e)}")
                raise ToolError(f"Failed to perform {action}: {str(e)}")

        elif action == "scroll":
            # Implement scroll action
            direction = kwargs.get("direction", "down")
            amount = kwargs.get("amount", 10)

            if direction not in ["up", "down"]:
                raise ToolError(f"Invalid scroll direction: {direction}. Must be 'up' or 'down'.")

            try:
                if direction == "down":
                    # Scroll down (Page Down on macOS)
                    self.logger.info(f"Scrolling down, amount: {amount}")
                    # Use fn+down for page down on macOS
                    for _ in range(amount):
                        await self.computer.interface.hotkey("fn", "down")
                        await asyncio.sleep(0.1)
                else:
                    # Scroll up (Page Up on macOS)
                    self.logger.info(f"Scrolling up, amount: {amount}")
                    # Use fn+up for page up on macOS
                    for _ in range(amount):
                        await self.computer.interface.hotkey("fn", "up")
                        await asyncio.sleep(0.1)

                # Wait briefly for UI changes
                await asyncio.sleep(0.5)

                # Take post-action screenshot
                post_screenshot = await self.computer.interface.screenshot()
                post_img = Image.open(io.BytesIO(post_screenshot))

                # Scale post-action image if needed
                if post_img.size != (self.width, self.height):
                    self.logger.info(
                        f"Scaling post-action image from {post_img.size} to {self.width}x{self.height}"
                    )
                    post_img = post_img.resize((self.width, self.height), Image.Resampling.LANCZOS)
                    buffer = io.BytesIO()
                    post_img.save(buffer, format="PNG")
                    post_screenshot = buffer.getvalue()

                return ToolResult(
                    output=f"Scrolled {direction} by {amount} steps",
                    base64_image=base64.b64encode(post_screenshot).decode(),
                )
            except Exception as e:
                self.logger.error(f"Error during scroll action: {str(e)}")
                raise ToolError(f"Failed to perform scroll: {str(e)}")

        raise ToolError(f"Invalid action: {action}")

    async def screenshot(self):
        """Take a screenshot and return it as a base64-encoded string."""
        try:
            screenshot = await self.computer.interface.screenshot()
            img = Image.open(io.BytesIO(screenshot))

            # Scale image if needed
            if img.size != (self.width, self.height):
                self.logger.info(f"Scaling image from {img.size} to {self.width}x{self.height}")
                if not isinstance(self.width, int) or not isinstance(self.height, int):
                    raise ToolError("Screen dimensions must be integers")
                size = (int(self.width), int(self.height))
                img = img.resize(size, Image.Resampling.LANCZOS)
                buffer = io.BytesIO()
                img.save(buffer, format="PNG")
                screenshot = buffer.getvalue()

            return ToolResult(base64_image=base64.b64encode(screenshot).decode())
        except Exception as e:
            self.logger.error(f"Error taking screenshot: {str(e)}")
            return ToolResult(error=f"Failed to take screenshot: {str(e)}")

    async def shell(self, command: str, take_screenshot=False) -> ToolResult:
        """Run a shell command and return the output, error, and optionally a screenshot."""
        try:
            _, stdout, stderr = await run(command)
            base64_image = None

            if take_screenshot:
                # delay to let things settle before taking a screenshot
                await asyncio.sleep(self._screenshot_delay)
                screenshot_result = await self.screenshot()
                if screenshot_result.error:
                    return ToolResult(
                        output=stdout,
                        error=f"{stderr}\nScreenshot error: {screenshot_result.error}",
                    )
                base64_image = screenshot_result.base64_image

            return ToolResult(output=stdout, error=stderr, base64_image=base64_image)

        except Exception as e:
            return ToolResult(error=f"Shell command failed: {str(e)}")
