#!/usr/bin/env python3
"""Diorama: A virtual desktop manager for macOS"""

import asyncio
import io
import logging
import os
import sys
from typing import Union

from computer_server.diorama.diorama_computer import DioramaComputer
from computer_server.diorama.draw import (
    AppActivationContext,
    capture_all_apps,
    get_all_windows,
    get_frontmost_and_active_app,
    get_running_apps,
)
from computer_server.handlers.macos import *
from PIL import Image, ImageDraw

# simple, nicely formatted logging
logger = logging.getLogger(__name__)

automation_handler = MacOSAutomationHandler()


class Diorama:
    """Virtual desktop manager that provides automation capabilities for macOS applications.

    Manages application windows and provides an interface for taking screenshots,
    mouse interactions, keyboard input, and coordinate transformations between
    screenshot space and screen space.
    """

    _scheduler_queue = None
    _scheduler_task = None
    _loop = None
    _scheduler_started = False

    @classmethod
    def create_from_apps(cls, *args) -> DioramaComputer:
        """Create a DioramaComputer instance from a list of application names.

        Args:
            *args: Variable number of application names to include in the desktop

        Returns:
            DioramaComputer: A computer interface for the specified applications
        """
        cls._ensure_scheduler()
        return cls(args).computer

    # Dictionary to store cursor positions for each unique app_list hash
    _cursor_positions = {}

    def __init__(self, app_list):
        """Initialize a Diorama instance for the specified applications.

        Args:
            app_list: List of application names to manage
        """
        self.app_list = app_list
        self.interface = self.Interface(self)
        self.computer = DioramaComputer(self)
        self.focus_context = None

        # Create a hash for this app_list to use as a key
        self.app_list_hash = hash(tuple(sorted(app_list)))

        # Initialize cursor position for this app_list if it doesn't exist
        if self.app_list_hash not in Diorama._cursor_positions:
            Diorama._cursor_positions[self.app_list_hash] = (0, 0)

    @classmethod
    def _ensure_scheduler(cls):
        """Ensure the async scheduler loop is running.

        Creates and starts the scheduler task if it hasn't been started yet.
        """
        if not cls._scheduler_started:
            logger.info("Starting Diorama scheduler loop…")
            cls._scheduler_queue = asyncio.Queue()
            cls._loop = asyncio.get_event_loop()
            cls._scheduler_task = cls._loop.create_task(cls._scheduler_loop())
            cls._scheduler_started = True

    @classmethod
    async def _scheduler_loop(cls):
        """Main scheduler loop that processes automation commands.

        Continuously processes commands from the scheduler queue, handling
        screenshots, mouse actions, keyboard input, and scrolling operations.
        """
        while True:
            cmd = await cls._scheduler_queue.get()
            action = cmd.get("action")
            args = cmd.get("arguments", {})
            future = cmd.get("future")
            logger.info(f"Processing command: {action} | args={args}")

            app_whitelist = args.get("app_list", [])

            all_windows = get_all_windows()
            running_apps = get_running_apps()
            frontmost_app, active_app_to_use, active_app_pid = get_frontmost_and_active_app(
                all_windows, running_apps, app_whitelist
            )
            focus_context = AppActivationContext(active_app_pid, active_app_to_use, logger)

            with focus_context:
                try:
                    if action == "screenshot":
                        logger.info(f"Taking screenshot for apps: {app_whitelist}")
                        result, img = capture_all_apps(
                            app_whitelist=app_whitelist, save_to_disk=False, take_focus=False
                        )
                        logger.info("Screenshot complete.")
                        if future:
                            future.set_result((result, img))
                    # Mouse actions
                    elif action in [
                        "left_click",
                        "right_click",
                        "double_click",
                        "move_cursor",
                        "drag_to",
                    ]:
                        x = args.get("x")
                        y = args.get("y")

                        duration = args.get("duration", 0.5)
                        if action == "left_click":
                            await automation_handler.left_click(x, y)
                        elif action == "right_click":
                            await automation_handler.right_click(x, y)
                        elif action == "double_click":
                            await automation_handler.double_click(x, y)
                        elif action == "move_cursor":
                            await automation_handler.move_cursor(x, y)
                        elif action == "drag_to":
                            await automation_handler.drag_to(x, y, duration=duration)
                        if future:
                            future.set_result(None)
                    elif action in ["scroll_up", "scroll_down"]:
                        x = args.get("x")
                        y = args.get("y")
                        if x is not None and y is not None:
                            await automation_handler.move_cursor(x, y)

                        clicks = args.get("clicks", 1)
                        if action == "scroll_up":
                            await automation_handler.scroll_up(clicks)
                        else:
                            await automation_handler.scroll_down(clicks)
                        if future:
                            future.set_result(None)
                    # Keyboard actions
                    elif action == "type_text":
                        text = args.get("text")
                        await automation_handler.type_text(text)
                        if future:
                            future.set_result(None)
                    elif action == "press_key":
                        key = args.get("key")
                        await automation_handler.press_key(key)
                        if future:
                            future.set_result(None)
                    elif action == "hotkey":
                        keys = args.get("keys", [])
                        await automation_handler.hotkey(keys)
                        if future:
                            future.set_result(None)
                    elif action == "get_cursor_position":
                        pos = await automation_handler.get_cursor_position()
                        if future:
                            future.set_result(pos)
                    else:
                        logger.warning(f"Unknown action: {action}")
                        if future:
                            future.set_exception(ValueError(f"Unknown action: {action}"))
                except Exception as e:
                    logger.error(f"Exception during {action}: {e}", exc_info=True)
                    if future:
                        future.set_exception(e)

    class Interface:
        """Interface for interacting with the virtual desktop.

        Provides methods for taking screenshots, mouse interactions, keyboard input,
        and coordinate transformations between screenshot and screen coordinates.
        """

        def __init__(self, diorama):
            """Initialize the interface with a reference to the parent Diorama instance.

            Args:
                diorama: The parent Diorama instance
            """
            self._diorama = diorama

            self._scene_hitboxes = []
            self._scene_size = None

        async def _send_cmd(self, action, arguments=None):
            """Send a command to the scheduler queue.

            Args:
                action (str): The action to perform
                arguments (dict, optional): Arguments for the action

            Returns:
                The result of the command execution
            """
            Diorama._ensure_scheduler()
            loop = asyncio.get_event_loop()
            future = loop.create_future()
            logger.info(f"Enqueuing {action} command for apps: {self._diorama.app_list}")
            await Diorama._scheduler_queue.put(
                {
                    "action": action,
                    "arguments": {"app_list": self._diorama.app_list, **(arguments or {})},
                    "future": future,
                }
            )
            try:
                return await future
            except asyncio.CancelledError:
                logger.warning(f"Command was cancelled: {action}")
                return None

        async def screenshot(self, as_bytes: bool = True) -> Union[str, Image.Image]:
            """Take a screenshot of the managed applications.

            Args:
                as_bytes (bool): If True, return base64-encoded bytes; if False, return PIL Image

            Returns:
                Union[str, Image.Image]: Base64-encoded PNG bytes or PIL Image object
            """
            import base64

            result, img = await self._send_cmd("screenshot")
            self._scene_hitboxes = result.get("hitboxes", [])
            self._scene_size = img.size

            if as_bytes:
                # PIL Image to bytes, then base64 encode for JSON
                import io

                img_byte_arr = io.BytesIO()
                img.save(img_byte_arr, format="PNG")
                img_bytes = img_byte_arr.getvalue()
                img_b64 = base64.b64encode(img_bytes).decode("ascii")
                return img_b64
            else:
                return img

        async def left_click(self, x, y):
            """Perform a left mouse click at the specified coordinates.

            Args:
                x (int): X coordinate in screenshot space (or None to use last position)
                y (int): Y coordinate in screenshot space (or None to use last position)
            """
            # Get last cursor position for this app_list hash
            app_list_hash = hash(tuple(sorted(self._diorama.app_list)))
            last_pos = Diorama._cursor_positions.get(app_list_hash, (0, 0))
            x, y = x or last_pos[0], y or last_pos[1]
            # Update cursor position for this app_list hash
            Diorama._cursor_positions[app_list_hash] = (x, y)

            sx, sy = await self.to_screen_coordinates(x, y)
            await self._send_cmd("left_click", {"x": sx, "y": sy})

        async def right_click(self, x, y):
            """Perform a right mouse click at the specified coordinates.

            Args:
                x (int): X coordinate in screenshot space (or None to use last position)
                y (int): Y coordinate in screenshot space (or None to use last position)
            """
            # Get last cursor position for this app_list hash
            app_list_hash = hash(tuple(sorted(self._diorama.app_list)))
            last_pos = Diorama._cursor_positions.get(app_list_hash, (0, 0))
            x, y = x or last_pos[0], y or last_pos[1]
            # Update cursor position for this app_list hash
            Diorama._cursor_positions[app_list_hash] = (x, y)

            sx, sy = await self.to_screen_coordinates(x, y)
            await self._send_cmd("right_click", {"x": sx, "y": sy})

        async def double_click(self, x, y):
            """Perform a double mouse click at the specified coordinates.

            Args:
                x (int): X coordinate in screenshot space (or None to use last position)
                y (int): Y coordinate in screenshot space (or None to use last position)
            """
            # Get last cursor position for this app_list hash
            app_list_hash = hash(tuple(sorted(self._diorama.app_list)))
            last_pos = Diorama._cursor_positions.get(app_list_hash, (0, 0))
            x, y = x or last_pos[0], y or last_pos[1]
            # Update cursor position for this app_list hash
            Diorama._cursor_positions[app_list_hash] = (x, y)

            sx, sy = await self.to_screen_coordinates(x, y)
            await self._send_cmd("double_click", {"x": sx, "y": sy})

        async def move_cursor(self, x, y):
            """Move the mouse cursor to the specified coordinates.

            Args:
                x (int): X coordinate in screenshot space (or None to use last position)
                y (int): Y coordinate in screenshot space (or None to use last position)
            """
            # Get last cursor position for this app_list hash
            app_list_hash = hash(tuple(sorted(self._diorama.app_list)))
            last_pos = Diorama._cursor_positions.get(app_list_hash, (0, 0))
            x, y = x or last_pos[0], y or last_pos[1]
            # Update cursor position for this app_list hash
            Diorama._cursor_positions[app_list_hash] = (x, y)

            sx, sy = await self.to_screen_coordinates(x, y)
            await self._send_cmd("move_cursor", {"x": sx, "y": sy})

        async def drag_to(self, x, y, duration=0.5):
            """Drag the mouse from current position to the specified coordinates.

            Args:
                x (int): X coordinate in screenshot space (or None to use last position)
                y (int): Y coordinate in screenshot space (or None to use last position)
                duration (float): Duration of the drag operation in seconds
            """
            # Get last cursor position for this app_list hash
            app_list_hash = hash(tuple(sorted(self._diorama.app_list)))
            last_pos = Diorama._cursor_positions.get(app_list_hash, (0, 0))
            x, y = x or last_pos[0], y or last_pos[1]
            # Update cursor position for this app_list hash
            Diorama._cursor_positions[app_list_hash] = (x, y)

            sx, sy = await self.to_screen_coordinates(x, y)
            await self._send_cmd("drag_to", {"x": sx, "y": sy, "duration": duration})

        async def get_cursor_position(self):
            """Get the current cursor position in screen coordinates.

            Returns:
                tuple: (x, y) coordinates of the cursor in screen space
            """
            return await self._send_cmd("get_cursor_position")

        async def type_text(self, text):
            """Type the specified text using the keyboard.

            Args:
                text (str): The text to type
            """
            await self._send_cmd("type_text", {"text": text})

        async def press_key(self, key):
            """Press a single key on the keyboard.

            Args:
                key (str): The key to press
            """
            await self._send_cmd("press_key", {"key": key})

        async def hotkey(self, keys):
            """Press a combination of keys simultaneously.

            Args:
                keys (list): List of keys to press together
            """
            await self._send_cmd("hotkey", {"keys": list(keys)})

        async def scroll_up(self, clicks: int = 1):
            """Scroll up at the current cursor position.

            Args:
                clicks (int): Number of scroll clicks to perform
            """
            # Get last cursor position for this app_list hash
            app_list_hash = hash(tuple(sorted(self._diorama.app_list)))
            last_pos = Diorama._cursor_positions.get(app_list_hash, (0, 0))
            x, y = last_pos[0], last_pos[1]

            await self._send_cmd("scroll_up", {"clicks": clicks, "x": x, "y": y})

        async def scroll_down(self, clicks: int = 1):
            """Scroll down at the current cursor position.

            Args:
                clicks (int): Number of scroll clicks to perform
            """
            # Get last cursor position for this app_list hash
            app_list_hash = hash(tuple(sorted(self._diorama.app_list)))
            last_pos = Diorama._cursor_positions.get(app_list_hash, (0, 0))
            x, y = last_pos[0], last_pos[1]

            await self._send_cmd("scroll_down", {"clicks": clicks, "x": x, "y": y})

        async def get_screen_size(self) -> dict[str, int]:
            """Get the size of the screenshot area.

            Returns:
                dict[str, int]: Dictionary with 'width' and 'height' keys
            """
            if not self._scene_size:
                await self.screenshot()
            return {"width": self._scene_size[0], "height": self._scene_size[1]}

        async def to_screen_coordinates(self, x: float, y: float) -> tuple[float, float]:
            """Convert screenshot coordinates to screen coordinates.

            Args:
                x: X absolute coordinate in screenshot space
                y: Y absolute coordinate in screenshot space

            Returns:
                tuple[float, float]: (x, y) absolute coordinates in screen space
            """
            if not self._scene_hitboxes:
                await self.screenshot()  # get hitboxes
            # Try all hitboxes
            for h in self._scene_hitboxes[::-1]:
                rect_from = h.get("hitbox")
                rect_to = h.get("target")
                if not rect_from or len(rect_from) != 4:
                    continue

                # check if (x, y) is inside rect_from
                x0, y0, x1, y1 = rect_from
                if x0 <= x <= x1 and y0 <= y <= y1:
                    logger.info(f"Found hitbox: {h}")
                    # remap (x, y) to rect_to
                    tx0, ty0, tx1, ty1 = rect_to

                    # calculate offset from x0, y0
                    offset_x = x - x0
                    offset_y = y - y0

                    # remap offset to rect_to
                    tx = tx0 + offset_x
                    ty = ty0 + offset_y

                    return tx, ty
            return x, y

        async def to_screenshot_coordinates(self, x: float, y: float) -> tuple[float, float]:
            """Convert screen coordinates to screenshot coordinates.

            Args:
                x: X absolute coordinate in screen space
                y: Y absolute coordinate in screen space

            Returns:
                tuple[float, float]: (x, y) absolute coordinates in screenshot space
            """
            if not self._scene_hitboxes:
                await self.screenshot()  # get hitboxes
            # Try all hitboxes
            for h in self._scene_hitboxes[::-1]:
                rect_from = h.get("target")
                rect_to = h.get("hitbox")
                if not rect_from or len(rect_from) != 4:
                    continue

                # check if (x, y) is inside rect_from
                x0, y0, x1, y1 = rect_from
                if x0 <= x <= x1 and y0 <= y <= y1:
                    # remap (x, y) to rect_to
                    tx0, ty0, tx1, ty1 = rect_to

                    # calculate offset from x0, y0
                    offset_x = x - x0
                    offset_y = y - y0

                    # remap offset to rect_to
                    tx = tx0 + offset_x
                    ty = ty0 + offset_y

                    return tx, ty
            return x, y


import time

from pynput.mouse import Controller as MouseController


async def main():
    """Main function demonstrating Diorama usage with multiple desktops and mouse tracking."""
    desktop1 = Diorama.create_from_apps(["Discord", "Notes"])
    desktop2 = Diorama.create_from_apps(["Terminal"])

    img1 = await desktop1.interface.screenshot(as_bytes=False)
    img2 = await desktop2.interface.screenshot(as_bytes=False)

    img1.save("app_screenshots/desktop1.png")
    img2.save("app_screenshots/desktop2.png")
    # Initialize Diorama desktop
    desktop3 = Diorama.create_from_apps("Safari")
    screen_size = await desktop3.interface.get_screen_size()
    print(screen_size)

    # Take initial screenshot
    img = await desktop3.interface.screenshot(as_bytes=False)
    img.save("app_screenshots/desktop3.png")

    # Prepare hitboxes and draw on the single screenshot
    hitboxes = desktop3.interface._scene_hitboxes[::-1]
    base_img = img.copy()
    draw = ImageDraw.Draw(base_img)
    for h in hitboxes:
        rect = h.get("hitbox")
        if not rect or len(rect) != 4:
            continue
        draw.rectangle(rect, outline="red", width=2)

    # Track and draw mouse position in real time (single screenshot size)
    last_mouse_pos = None
    print("Tracking mouse... Press Ctrl+C to stop.")
    try:
        mouse = MouseController()
        while True:
            mx, my = mouse.position
            mouse_x, mouse_y = int(mx), int(my)
            if last_mouse_pos != (mouse_x, mouse_y):
                last_mouse_pos = (mouse_x, mouse_y)
                # Map to screenshot coordinates
                sx, sy = await desktop3.interface.to_screenshot_coordinates(mouse_x, mouse_y)
                # Draw on a copy of the screenshot
                frame = base_img.copy()
                frame_draw = ImageDraw.Draw(frame)
                frame_draw.ellipse((sx - 5, sy - 5, sx + 5, sy + 5), fill="blue", outline="blue")
                # Save the frame
                frame.save("app_screenshots/desktop3_mouse.png")
                print(f"Mouse at screen ({mouse_x}, {mouse_y}) -> screenshot ({sx:.1f}, {sy:.1f})")
            time.sleep(0.05)  # Throttle updates to ~20 FPS
    except KeyboardInterrupt:
        print("Stopped tracking.")

        draw.text((rect[0], rect[1]), str(idx), fill="red")

    canvas.save("app_screenshots/desktop3_hitboxes.png")

    # move mouse in a square spiral around the screen
    import math
    import random

    step = 20  # pixels per move
    dot_radius = 10
    width = screen_size["width"]
    height = screen_size["height"]
    x, y = 0, 10

    while x < width and y < height:
        await desktop3.interface.move_cursor(x, y)
        img = await desktop3.interface.screenshot(as_bytes=False)
        draw = ImageDraw.Draw(img)
        draw.ellipse((x - dot_radius, y - dot_radius, x + dot_radius, y + dot_radius), fill="red")
        img.save("current.png")
        await asyncio.sleep(0.03)
        x += step
        y = math.sin(x / width * math.pi * 2) * 50 + 25


if __name__ == "__main__":
    asyncio.run(main())
