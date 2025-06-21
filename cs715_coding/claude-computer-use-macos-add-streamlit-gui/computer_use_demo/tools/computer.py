import asyncio
import base64
import io
from enum import StrEnum
from typing import Literal, TypedDict
import pyautogui
from anthropic.types.beta import BetaToolComputerUse20241022Param

from .base import BaseAnthropicTool, ToolError, ToolResult

OUTPUT_DIR = "/tmp/outputs"

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
]


class ScalingSource(StrEnum):
    COMPUTER = "computer"
    API = "api"


class ComputerToolOptions(TypedDict):
    display_height_px: int
    display_width_px: int
    display_number: int | None


def chunks(s: str, chunk_size: int) -> list[str]:
    return [s[i : i + chunk_size] for i in range(0, len(s), chunk_size)]


class ComputerTool(BaseAnthropicTool):
    """
    A tool that allows the agent to interact with the screen, keyboard, and mouse of the current computer.
    The tool parameters are defined by Anthropic and are not editable.
    """

    name: Literal["computer"] = "computer"
    api_type: Literal["computer_20241022"] = "computer_20241022"
    width: int
    height: int
    display_num: int | None

    _screenshot_delay = 1.0
    _scaling_enabled = True

    @property
    def options(self) -> ComputerToolOptions:
        return {
            "display_width_px": self.target_width,
            "display_height_px": self.target_height,
            "display_number": self.display_num,
        }

    def to_params(self) -> BetaToolComputerUse20241022Param:
        return {"name": self.name, "type": self.api_type, **self.options}

    def __init__(self):
        super().__init__()

        self.width = int(pyautogui.size()[0])
        self.height = int(pyautogui.size()[1])

        self.display_num = None  # Not used on MacOS

        MAX_WIDTH = 1280  # Max screenshot width
        if self.width > MAX_WIDTH:
            self.scale_factor = MAX_WIDTH / self.width
            self.target_width = MAX_WIDTH
            self.target_height = int(self.height * self.scale_factor)
        else:
            self.scale_factor = 1.0
            self.target_width = self.width
            self.target_height = self.height

    async def __call__(
        self,
        *,
        action: Action,
        text: str | None = None,
        coordinate: list[int] | None = None,
        **kwargs,
    ):
        print(
            f"### Performing action: {action}{f', text: {text}' if text else ''}{f', coordinate: {coordinate}' if coordinate else ''}"
        )
        if action in ("mouse_move", "left_click_drag"):
            if coordinate is None:
                raise ToolError(f"coordinate is required for {action}")
            if text is not None:
                raise ToolError(f"text is not accepted for {action}")
            if not isinstance(coordinate, list) or len(coordinate) != 2:
                raise ToolError(f"coordinate must be a list of length 2")
            if not all(isinstance(i, int) and i >= 0 for i in coordinate):
                raise ToolError(f"coordinate must be a list of non-negative integers")

            x, y = self.scale_coordinates(
                ScalingSource.API, coordinate[0], coordinate[1]
            )

            if action == "mouse_move":
                await asyncio.to_thread(pyautogui.moveTo, x, y)
                return ToolResult(output=f"Mouse moved successfully to X={x}, Y={y}")
            elif action == "left_click_drag":
                await asyncio.to_thread(pyautogui.mouseDown)
                await asyncio.to_thread(pyautogui.moveTo, x, y)
                await asyncio.to_thread(pyautogui.mouseUp)
                return ToolResult(output="Mouse drag action completed.")

        if action in ("key", "type"):
            if text is None:
                raise ToolError(f"text is required for {action}")
            if coordinate is not None:
                raise ToolError(f"coordinate is not accepted for {action}")
            if not isinstance(text, str):
                raise ToolError(f"text must be a string")

            if action == "key":
                # Handle key combinations and modifiers
                # Replace 'super' with 'command'
                key_sequence = text.lower().replace("super", "command").split("+")
                key_sequence = [key.strip() for key in key_sequence]
                # Map 'cmd' to 'command' for MacOS
                key_sequence = [
                    "command" if key == "cmd" else key for key in key_sequence
                ]
                # Handle special keys that pyautogui expects
                special_keys = {
                    "ctrl": "ctrl",
                    "control": "ctrl",
                    "alt": "alt",
                    "option": "alt",
                    "shift": "shift",
                    "command": "command",
                    "tab": "tab",
                    "enter": "enter",
                    "return": "enter",
                    "esc": "esc",
                    "escape": "esc",
                    "space": "space",
                    "spacebar": "space",
                    "up": "up",
                    "down": "down",
                    "left": "left",
                    "right": "right",
                    # Add more special keys as needed
                }
                key_sequence = [special_keys.get(key, key) for key in key_sequence]
                await asyncio.to_thread(pyautogui.hotkey, *key_sequence)
                return ToolResult(output=f"Key combination '{text}' pressed.")
            elif action == "type":
                await asyncio.to_thread(
                    pyautogui.write, text, interval=TYPING_DELAY_MS / 1000.0
                )
                return ToolResult(output=f"Typed text: {text}")

        if action in (
            "left_click",
            "right_click",
            "double_click",
            "screenshot",
            "cursor_position",
        ):
            if text is not None:
                raise ToolError(f"text is not accepted for {action}")
            if coordinate is not None:
                raise ToolError(f"coordinate is not accepted for {action}")

            if action == "screenshot":
                return await self.screenshot()
            elif action == "cursor_position":
                x, y = pyautogui.position()
                x, y = self.scale_coordinates(ScalingSource.COMPUTER, int(x), int(y))
                return ToolResult(output=f"X={x},Y={y}")
            else:
                if action == "left_click":
                    await asyncio.to_thread(pyautogui.click, button="left")
                    return ToolResult(output="Left click performed.")
                elif action == "right_click":
                    await asyncio.to_thread(pyautogui.click, button="right")
                    return ToolResult(output="Right click performed.")
                elif action == "double_click":
                    await asyncio.to_thread(pyautogui.doubleClick)
                    return ToolResult(output="Double click performed.")

        raise ToolError(f"Invalid action: {action}")

    async def screenshot(self):
        """Take a screenshot of the current screen and return the base64 encoded image."""
        # Capture screenshot using PyAutoGUI
        screenshot = await asyncio.to_thread(pyautogui.screenshot)

        if self._scaling_enabled and self.scale_factor < 1.0:
            screenshot = screenshot.resize((self.target_width, self.target_height))

        img_buffer = io.BytesIO()
        # Save the image to an in-memory buffer
        screenshot.save(img_buffer, format="PNG", optimize=True)
        img_buffer.seek(0)
        base64_image = base64.b64encode(img_buffer.read()).decode()

        return ToolResult(base64_image=base64_image)

    def scale_coordinates(self, source: ScalingSource, x: int, y: int):
        """Scale coordinates between the assistant's coordinate system and the real screen coordinates."""
        if not self._scaling_enabled:
            return x, y
        x_scaling_factor = self.width / self.target_width
        y_scaling_factor = self.height / self.target_height
        if source == ScalingSource.API:
            # Assistant's coordinates -> real screen coordinates
            return round(x * x_scaling_factor), round(y * y_scaling_factor)
        else:
            # Real screen coordinates -> assistant's coordinate system
            return round(x / x_scaling_factor), round(y / y_scaling_factor)
