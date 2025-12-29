import asyncio

from .interface.models import Key, KeyType


class DioramaComputer:
    """
    A Computer-compatible proxy for Diorama that sends commands over the ComputerInterface.
    """

    def __init__(self, computer, apps):
        """
        Initialize the DioramaComputer with a computer instance and list of apps.

        Args:
            computer: The computer instance to proxy commands through
            apps: List of applications available in the diorama environment
        """
        self.computer = computer
        self.apps = apps
        self.interface = DioramaComputerInterface(computer, apps)
        self._initialized = False

    async def __aenter__(self):
        """
        Async context manager entry point.

        Returns:
            self: The DioramaComputer instance
        """
        self._initialized = True
        return self

    async def run(self):
        """
        Initialize and run the DioramaComputer if not already initialized.

        Returns:
            self: The DioramaComputer instance
        """
        if not self._initialized:
            await self.__aenter__()
        return self


class DioramaComputerInterface:
    """
    Diorama Interface proxy that sends diorama_cmds via the Computer's interface.
    """

    def __init__(self, computer, apps):
        """
        Initialize the DioramaComputerInterface.

        Args:
            computer: The computer instance to send commands through
            apps: List of applications available in the diorama environment
        """
        self.computer = computer
        self.apps = apps
        self._scene_size = None

    async def _send_cmd(self, action, arguments=None):
        """
        Send a command to the diorama interface through the computer.

        Args:
            action (str): The action/command to execute
            arguments (dict, optional): Additional arguments for the command

        Returns:
            The result from the diorama command execution

        Raises:
            RuntimeError: If the computer interface is not initialized or command fails
        """
        arguments = arguments or {}
        arguments = {"app_list": self.apps, **arguments}
        # Use the computer's interface (must be initialized)
        iface = getattr(self.computer, "_interface", None)
        if iface is None:
            raise RuntimeError("Computer interface not initialized. Call run() first.")
        result = await iface.diorama_cmd(action, arguments)
        if not result.get("success"):
            raise RuntimeError(
                f"Diorama command failed: {result.get('error')}\n{result.get('trace')}"
            )
        return result.get("result")

    async def screenshot(self, as_bytes=True):
        """
        Take a screenshot of the diorama scene.

        Args:
            as_bytes (bool): If True, return image as bytes; if False, return PIL Image object

        Returns:
            bytes or PIL.Image: Screenshot data in the requested format
        """
        import base64

        from PIL import Image

        result = await self._send_cmd("screenshot")
        # assume result is a b64 string of an image
        img_bytes = base64.b64decode(result)
        import io

        img = Image.open(io.BytesIO(img_bytes))
        self._scene_size = img.size
        return img_bytes if as_bytes else img

    async def get_screen_size(self):
        """
        Get the dimensions of the diorama scene.

        Returns:
            dict: Dictionary containing 'width' and 'height' keys with pixel dimensions
        """
        if not self._scene_size:
            await self.screenshot(as_bytes=False)
        return {"width": self._scene_size[0], "height": self._scene_size[1]}

    async def move_cursor(self, x, y):
        """
        Move the cursor to the specified coordinates.

        Args:
            x (int): X coordinate to move cursor to
            y (int): Y coordinate to move cursor to
        """
        await self._send_cmd("move_cursor", {"x": x, "y": y})

    async def left_click(self, x=None, y=None):
        """
        Perform a left mouse click at the specified coordinates or current cursor position.

        Args:
            x (int, optional): X coordinate to click at. If None, clicks at current cursor position
            y (int, optional): Y coordinate to click at. If None, clicks at current cursor position
        """
        await self._send_cmd("left_click", {"x": x, "y": y})

    async def right_click(self, x=None, y=None):
        """
        Perform a right mouse click at the specified coordinates or current cursor position.

        Args:
            x (int, optional): X coordinate to click at. If None, clicks at current cursor position
            y (int, optional): Y coordinate to click at. If None, clicks at current cursor position
        """
        await self._send_cmd("right_click", {"x": x, "y": y})

    async def double_click(self, x=None, y=None):
        """
        Perform a double mouse click at the specified coordinates or current cursor position.

        Args:
            x (int, optional): X coordinate to double-click at. If None, clicks at current cursor position
            y (int, optional): Y coordinate to double-click at. If None, clicks at current cursor position
        """
        await self._send_cmd("double_click", {"x": x, "y": y})

    async def scroll_up(self, clicks=1):
        """
        Scroll up by the specified number of clicks.

        Args:
            clicks (int): Number of scroll clicks to perform upward. Defaults to 1
        """
        await self._send_cmd("scroll_up", {"clicks": clicks})

    async def scroll_down(self, clicks=1):
        """
        Scroll down by the specified number of clicks.

        Args:
            clicks (int): Number of scroll clicks to perform downward. Defaults to 1
        """
        await self._send_cmd("scroll_down", {"clicks": clicks})

    async def drag_to(self, x, y, duration=0.5):
        """
        Drag from the current cursor position to the specified coordinates.

        Args:
            x (int): X coordinate to drag to
            y (int): Y coordinate to drag to
            duration (float): Duration of the drag operation in seconds. Defaults to 0.5
        """
        await self._send_cmd("drag_to", {"x": x, "y": y, "duration": duration})

    async def get_cursor_position(self):
        """
        Get the current cursor position.

        Returns:
            dict: Dictionary containing the current cursor coordinates
        """
        return await self._send_cmd("get_cursor_position")

    async def type_text(self, text):
        """
        Type the specified text at the current cursor position.

        Args:
            text (str): The text to type
        """
        await self._send_cmd("type_text", {"text": text})

    async def press_key(self, key):
        """
        Press a single key.

        Args:
            key: The key to press
        """
        await self._send_cmd("press_key", {"key": key})

    async def hotkey(self, *keys):
        """
        Press multiple keys simultaneously as a hotkey combination.

        Args:
            *keys: Variable number of keys to press together. Can be Key enum instances or strings

        Raises:
            ValueError: If any key is not a Key enum or string type
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
        await self._send_cmd("hotkey", {"keys": actual_keys})

    async def to_screen_coordinates(self, x, y):
        """
        Convert coordinates to screen coordinates.

        Args:
            x (int): X coordinate to convert
            y (int): Y coordinate to convert

        Returns:
            dict: Dictionary containing the converted screen coordinates
        """
        return await self._send_cmd("to_screen_coordinates", {"x": x, "y": y})
